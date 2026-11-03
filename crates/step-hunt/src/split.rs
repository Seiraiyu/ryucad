//! Sources into cases: every body of every file split out (see
//! [`crate::body`]), identical bodies one case, and each file whole where
//! its source asks, or where it can't be split.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::body;
use crate::sources::{self, Source};

/// Where a case came from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Origin {
    pub source: String,
    /// The file, relative to its source's root.
    pub file: String,
    /// The representation's STEP instance in that file; 0 for a whole file.
    pub representation: u64,
    /// The product whose shape it is, by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// One body split out: run through the body stages.
    Body,
    /// A whole file: run through the whole-file stages.
    Whole,
    /// A file that can't be run (its bytes aren't what its source listed):
    /// one stage, `open`, failing with `refused`.
    Open,
}

/// One case: a file to run, by the hash of its bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub kind: Kind,
    /// Its bytes, written once under the work folder.
    pub path: PathBuf,
    pub faces: usize,
    pub surfaces: Vec<String>,
    /// For a whole file: the file couldn't be split into bodies (it
    /// doesn't parse or decode, or names an instance it lacks).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsplit: bool,
    /// For an `open` case: the error its `open` stage fails with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<serde_json::Value>,
    pub origins: Vec<Origin>,
}

/// What splitting the sources found.
#[derive(Debug, Default)]
pub struct Split {
    /// By id.
    pub cases: BTreeMap<String, Case>,
    /// Sources whose root is missing, by name.
    pub missing: Vec<String>,
    /// Why sources were skipped, and what pulling them said.
    pub notes: Vec<String>,
    /// The `cox-files:` sources pulled, by name.
    pub pulled: Vec<(String, crate::cox_files::Pulled)>,
}

struct Found {
    id: String,
    kind: Kind,
    path: PathBuf,
    faces: usize,
    surfaces: Vec<String>,
    unsplit: bool,
    refused: Option<serde_json::Value>,
    origin: Origin,
}

/// Writes `bytes` to `cases/<id>.step` under `work` unless it is there.
/// Two threads may store one body at once: each writes a file of its own
/// and renames it into place, and the bytes are the same.
fn store(work: &Path, bytes: &[u8]) -> std::io::Result<(String, PathBuf)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = body::case_id(bytes);
    let path = work.join("cases").join(format!("{id}.step"));
    if !path.exists() {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("{}-{n}.part", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok((id, path))
}

/// The cases one file gives.
fn split_file(
    source: &Source,
    rel: &str,
    bytes: &[u8],
    work: &Path,
) -> std::io::Result<Vec<Found>> {
    let origin = |representation, product| Origin {
        source: source.name.clone(),
        file: rel.to_owned(),
        representation,
        product,
    };
    let parsed = ryu_part21::parse(bytes).ok();
    let whole = |unsplit: bool| -> std::io::Result<Found> {
        let (id, path) = store(work, bytes)?;
        let (faces, surfaces) = match &parsed {
            Some(f) => (body::faces(f), body::surfaces(f)),
            None => (0, Vec::new()),
        };
        Ok(Found {
            id,
            kind: Kind::Whole,
            path,
            faces,
            surfaces,
            unsplit,
            refused: None,
            origin: origin(0, None),
        })
    };
    let (Some(file), Ok(opened)) = (&parsed, ryustep::open(bytes)) else {
        return Ok(vec![whole(true)?]);
    };
    let mut out = Vec::new();
    for h in &opened.held {
        let product = h
            .definition
            .map(|k| opened.products.definitions[k].name.to_string());
        match body::body_file(file, h.representation, h.context, &h.items) {
            Ok(b) => {
                let parsed = ryu_part21::parse(&b).expect("a body file reads back");
                let (id, path) = store(work, &b)?;
                out.push(Found {
                    id,
                    kind: Kind::Body,
                    path,
                    faces: body::faces(&parsed),
                    surfaces: body::surfaces(&parsed),
                    unsplit: false,
                    refused: None,
                    origin: origin(h.representation, product),
                });
            }
            Err(_) => return Ok(vec![whole(true)?]),
        }
    }
    if source.whole {
        out.push(whole(false)?);
    }
    Ok(out)
}

/// The `open` case of a pulled file whose bytes aren't what `cox` listed.
fn mismatched(
    s: &Source,
    p: &crate::cox_files::Pulled,
    m: &crate::cox_files::Mismatch,
    work: &Path,
) -> Result<Found, String> {
    let bytes = std::fs::read(&m.file).map_err(|e| format!("{}: {e}", m.file.display()))?;
    let (id, path) = store(work, &bytes).map_err(|e| format!("{}: {e}", work.display()))?;
    Ok(Found {
        id,
        kind: Kind::Open,
        path,
        faces: 0,
        surfaces: Vec::new(),
        unsplit: true,
        refused: Some(serde_json::json!({
            "code": "cox_files_hash_mismatch",
            "message": format!("{}@v{}: {}'s bytes aren't the SHA-256 cox lists", p.set, m.version, m.name),
            "details": {"set": p.set, "version": m.version, "file": m.name, "expected": m.expected, "found": m.found},
        })),
        origin: Origin {
            source: s.name.clone(),
            file: m.name.clone(),
            representation: 0,
            product: None,
        },
    })
}

/// Splits every file of `sources` (relative to `base`) into cases under
/// `work`. A source whose root is missing is listed in `missing`.
pub fn split(sources: &[Source], base: &Path, work: &Path) -> Result<Split, String> {
    std::fs::create_dir_all(work.join("cases")).map_err(|e| format!("{}: {e}", work.display()))?;
    let mut out = Split::default();
    let mut jobs: Vec<(&Source, PathBuf, String)> = Vec::new();
    let cox = crate::cox_files::Cox::from_env();
    let links = sources::expand("${HUNT_COX_FILES:-target/cox-files}", base)?;
    let mut refused: Vec<Found> = Vec::new();
    for s in sources {
        let root = match crate::cox_files::Root::parse(&s.root) {
            None => sources::expand(&s.root, base)?,
            Some(r) => match crate::cox_files::materialize(&cox, &r?, &links) {
                Err(e) => {
                    out.missing.push(s.name.clone());
                    out.notes.push(format!("source {}: {e}", s.name));
                    continue;
                }
                Ok(p) => {
                    out.notes
                        .extend(p.notes.iter().map(|n| format!("source {}: {n}", s.name)));
                    for m in &p.mismatched {
                        refused.push(mismatched(s, &p, m, work)?);
                    }
                    let root = p.root.clone();
                    out.pulled.push((s.name.clone(), p));
                    root
                }
            },
        };
        if !root.is_dir() {
            out.missing.push(s.name.clone());
            continue;
        }
        let files =
            sources::files(&root, &s.glob).map_err(|e| format!("{}: {e}", root.display()))?;
        jobs.extend(files.into_iter().map(|f| (s, root.join(&f), f)));
    }
    let found: Vec<Found> = jobs
        .par_iter()
        .map(|(s, path, rel)| {
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            split_file(s, rel, &bytes, work).map_err(|e| format!("{}: {e}", work.display()))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .flatten()
        .chain(refused)
        .collect();
    for f in found {
        let case = out.cases.entry(f.id.clone()).or_insert_with(|| Case {
            id: f.id,
            kind: f.kind,
            path: f.path,
            faces: f.faces,
            surfaces: f.surfaces,
            unsplit: f.unsplit,
            refused: f.refused,
            origins: Vec::new(),
        });
        case.origins.push(f.origin);
    }
    for c in out.cases.values_mut() {
        c.origins.sort();
        c.origins.dedup();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::tests::sample;

    fn source(name: &str, root: &Path, whole: bool) -> Source {
        Source {
            name: name.into(),
            root: root.to_string_lossy().into_owned(),
            glob: "*.stp".into(),
            licence: "LicenseRef-test".into(),
            whole,
        }
    }

    #[test]
    fn copies_across_files_are_one_case_with_both_origins() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.stp"), sample(10)).unwrap();
        std::fs::write(src.join("b.stp"), sample(700)).unwrap();
        std::fs::write(src.join("junk.stp"), "not a step file").unwrap();
        let s = split(
            &[
                source("t", &src, false),
                source("gone", &dir.path().join("nope"), false),
            ],
            dir.path(),
            &dir.path().join("work"),
        )
        .unwrap();
        assert_eq!(s.missing, ["gone"]);
        let bodies: Vec<&Case> = s.cases.values().filter(|c| c.kind == Kind::Body).collect();
        assert_eq!(bodies.len(), 1);
        let files: Vec<&str> = bodies[0].origins.iter().map(|o| o.file.as_str()).collect();
        assert_eq!(files, ["a.stp", "b.stp"]);
        assert_eq!(bodies[0].faces, 1);
        assert!(bodies[0].path.exists());
        let wholes: Vec<&Case> = s.cases.values().filter(|c| c.kind == Kind::Whole).collect();
        assert_eq!(wholes.len(), 1);
        assert!(wholes[0].unsplit);
        assert_eq!(wholes[0].origins[0].file, "junk.stp");
    }

    #[test]
    fn a_whole_source_also_runs_each_file_whole() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.stp"), sample(10)).unwrap();
        let s = split(
            &[source("t", dir.path(), true)],
            dir.path(),
            &dir.path().join("work"),
        )
        .unwrap();
        let kinds: Vec<Kind> = s.cases.values().map(|c| c.kind).collect();
        assert_eq!(kinds.iter().filter(|k| **k == Kind::Whole).count(), 1);
        assert_eq!(kinds.iter().filter(|k| **k == Kind::Body).count(), 1);
    }
}
