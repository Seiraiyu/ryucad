//! External-reference assemblies (design 448, plan 24.D, #443): an
//! assembly file whose products' shapes are in other STEP files, each
//! named by a document (`DOCUMENT_FILE`, a digital `DOCUMENT`, OCCT's
//! `cad_filename`), as ryustep reads them (`Definition::external`).
//!
//! [`resolve`] finds and opens every file an import reads, depth first
//! from the file given: each externally defined definition's documents
//! relative to the referencing file's folder (CAx-IF's rule), then by
//! file name in each `--search` folder. Files with the same bytes are one
//! [`Node`]. Nothing is written until every file is found:
//! `import_external_missing` lists every document that didn't resolve.
//! [`Planner`] then plans each file as an import of its own, leaves
//! first, the referencing definitions placing the parts their files make.

use ryustep::products::External;
use std::path::{Path, PathBuf};

/// A document's names as written, `\` read as `/`: its location, then
/// its name.
fn written(x: &External) -> Vec<String> {
    x.location
        .iter()
        .chain(std::iter::once(&x.name))
        .map(|s| s.trim().replace('\\', "/"))
        .filter(|s| !s.is_empty())
        .collect()
}

/// A document's file names alone (each written name's last segment), in
/// order, once each.
fn file_names(x: &External) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in written(x) {
        if let Some(n) = w.rsplit('/').next().filter(|n| !n.is_empty())
            && !out.iter().any(|o| o == n)
        {
            out.push(n.to_owned());
        }
    }
    out
}

/// Whether a written path is absolute on some system: `/…` or `C:…`.
fn absolute(w: &str) -> bool {
    let b = w.as_bytes();
    w.starts_with('/') || (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic())
}

/// Where a document may be, in order: its location, then its name,
/// relative to `folder` (each unless written absolute); then its file
/// name alone in `folder` and in each of `search`.
pub fn candidates(x: &External, folder: &Path, search: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for w in written(x) {
        if !absolute(&w) {
            push(folder.join(w.trim_start_matches("./")));
        }
    }
    let names = file_names(x);
    for n in &names {
        push(folder.join(n));
    }
    for dir in search {
        for n in &names {
            push(dir.join(n));
        }
    }
    out
}

/// The file a document is: the first of [`candidates`] that is a file,
/// else, in `folder` and then each of `search`, the one file whose name
/// matches a file name of it ignoring ASCII case.
pub fn locate(x: &External, folder: &Path, search: &[PathBuf]) -> Option<PathBuf> {
    if let Some(p) = candidates(x, folder, search).into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    let names = file_names(x);
    for dir in std::iter::once(folder).chain(search.iter().map(PathBuf::as_path)) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut hits: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                names.iter().any(|w| w.eq_ignore_ascii_case(&n))
            })
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        hits.sort();
        if let [p] = &hits[..] {
            return Some(p.clone());
        }
    }
    None
}

/// Whether a document's format, as its writer names it, is one ryucad
/// reads: unnamed, or naming STEP, Part 21 or ISO 10303.
pub fn is_step(format: Option<&str>) -> bool {
    format.filter(|f| !f.trim().is_empty()).is_none_or(|f| {
        let l = f.to_ascii_lowercase();
        ["step", "stp", "10303", "p21", "part 21", "part21"]
            .iter()
            .any(|s| l.contains(s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, location: Option<&str>) -> External {
        External {
            name: name.into(),
            location: location.map(str::to_owned),
            format: None,
            instance: 1,
            related: vec![1],
        }
    }

    #[test]
    fn a_document_is_looked_for_by_location_name_then_file_name() {
        let f = Path::new("/a/asm");
        let s = [PathBuf::from("/s1"), PathBuf::from("/s2")];
        assert_eq!(
            candidates(&doc("sub\\x.stp", Some("parts/x.stp")), f, &s),
            [
                PathBuf::from("/a/asm/parts/x.stp"),
                PathBuf::from("/a/asm/sub/x.stp"),
                PathBuf::from("/a/asm/x.stp"),
                PathBuf::from("/s1/x.stp"),
                PathBuf::from("/s2/x.stp"),
            ]
        );
        // Absolute names from another machine: by file name only.
        assert_eq!(
            candidates(&doc("C:\\Users\\me\\Part1.STEP", None), f, &[]),
            [PathBuf::from("/a/asm/Part1.STEP")]
        );
        assert_eq!(
            candidates(&doc("/home/me/p.stp", None), f, &[]),
            [PathBuf::from("/a/asm/p.stp")]
        );
    }

    #[test]
    fn a_file_is_found_exactly_else_by_one_name_ignoring_case() {
        let d = crate::import::tests::folder(&[
            ("a/one.stp", b"1".to_vec()),
            ("s/Part1.step", b"2".to_vec()),
        ]);
        let a = d.path().join("a");
        let s = [d.path().join("s")];
        assert_eq!(locate(&doc("one.stp", None), &a, &s), Some(a.join("one.stp")));
        assert_eq!(
            locate(&doc("PART1.STEP", None), &a, &s),
            Some(s[0].join("Part1.step"))
        );
        assert_eq!(locate(&doc("none.stp", None), &a, &s), None);
    }

    #[test]
    fn formats_other_than_step_are_not_read() {
        assert!(is_step(None));
        assert!(is_step(Some("")));
        assert!(is_step(Some("ISO 10303-21")));
        assert!(is_step(Some("STEP AP214")));
        assert!(!is_step(Some("CATIA V5")));
        assert!(!is_step(Some("SLDPRT")));
    }
}
