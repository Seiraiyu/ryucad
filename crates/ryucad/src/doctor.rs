//! Diagnosing and repairing a project folder. A folder can break in ways a
//! single file can't: a lost manifest, a feature file nobody lists, Git
//! conflict markers, a sync tool's conflicted copy. `doctor` reports each
//! with a stable code, and never changes anything itself.
//!
//! It reads the folder directly, not through [`crate::doc::project`], so
//! it works on a project that can't be opened.

use crate::doc::canonical;
use crate::doc::files::{Feature, Manifest, PartFile};
use crate::doc::id::Id;
use crate::doc::project::MANIFEST;
use crate::error::{Code, Result};
use crate::units::Unit;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// One problem, as reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// A stable code: see [`CODES`].
    pub code: &'static str,
    /// `error` (the project or a part can't be used as it is) or `warning`.
    pub severity: &'static str,
    /// The files involved, relative to the root.
    pub files: Vec<String>,
    pub message: String,
}

impl Issue {
    pub fn to_json(&self) -> Value {
        json!({"code": self.code, "severity": self.severity, "files": self.files,
               "message": self.message})
    }
}

/// Every issue code, each with its severity.
pub const CODES: [(&str, &str); 14] = [
    ("missing_manifest", "error"),
    ("missing_part_index", "error"),
    ("listed_feature_missing", "error"),
    ("orphan_feature", "warning"),
    ("invalid_json", "error"),
    ("invalid_document", "error"),
    ("conflict_markers", "error"),
    ("sync_conflict_copy", "warning"),
    ("invalid_file_name", "warning"),
    ("case_collision", "error"),
    ("unknown_file", "warning"),
    ("non_canonical", "warning"),
    ("format_too_new", "error"),
    ("cloud_sync_folder", "warning"),
];

fn issue(code: &'static str, files: Vec<String>, message: String) -> Issue {
    let severity = CODES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, s)| *s)
        .expect("a known code");
    Issue {
        code,
        severity,
        files,
        message,
    }
}

/// What a file holds, as far as doctor is concerned.
enum Read {
    Missing,
    /// It parsed: the value, and whether it was canonical.
    Json(Value, bool),
    /// It didn't; the issue says why.
    Bad(Issue),
}

/// Whether a file holds Git conflict markers.
fn has_markers(text: &str) -> bool {
    let line = |p: &str| text.lines().any(|l| l.starts_with(p));
    line("<<<<<<< ") && line(">>>>>>> ")
}

fn read(root: &Path, rel: &str) -> Read {
    let path = root.join(rel);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) if !path.exists() => return Read::Missing,
        Err(e) => {
            return Read::Bad(issue(
                "unknown_file",
                vec![rel.to_owned()],
                format!("{rel} can't be read: {e}"),
            ));
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Read::Bad(issue(
            "invalid_json",
            vec![rel.to_owned()],
            format!("{rel} is not UTF-8"),
        ));
    };
    if has_markers(&text) {
        return Read::Bad(issue(
            "conflict_markers",
            vec![rel.to_owned()],
            format!(
                "{rel} has Git conflict markers: a merge is unfinished; resolve it, then commit"
            ),
        ));
    }
    match canonical::parse(&text) {
        Ok(v) => Read::Json(v, canonical::is_canonical(&text)),
        Err(e) => Read::Bad(issue(
            "invalid_json",
            vec![rel.to_owned()],
            format!("{rel}: {}", e.message),
        )),
    }
}

/// The issue for a document that parsed but doesn't hold what it should.
fn document_issue(rel: &str, e: &crate::error::Error) -> Issue {
    let code = if e.code == Code::FormatTooNew {
        "format_too_new"
    } else {
        "invalid_document"
    };
    issue(code, vec![rel.to_owned()], format!("{rel}: {}", e.message))
}

/// The names in a folder, sorted; empty when it doesn't exist.
fn names(dir: &Path) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        e.path().is_dir(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// If `name` (without `.json`) looks like a sync tool's copy of one of
/// `originals`, the original: `boss (1)`, `boss (Dana's conflicted copy
/// 2026-10-16)`, `boss - Copy`, `boss copy 2`, `boss-DESKTOP-1A2B`.
pub fn copy_of<'a>(stem: &str, originals: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    for o in originals {
        let Some(rest) = stem.strip_prefix(*o) else {
            continue;
        };
        let paren = rest.starts_with(" (") && rest.ends_with(')');
        let windows = rest == " - Copy"
            || rest.strip_prefix(" - Copy (").is_some_and(|r| {
                r.strip_suffix(')')
                    .is_some_and(|n| n.parse::<u32>().is_ok())
            });
        let mac = rest == " copy"
            || rest
                .strip_prefix(" copy ")
                .is_some_and(|n| n.parse::<u32>().is_ok());
        let host = rest.strip_prefix('-').is_some_and(|h| {
            !h.is_empty()
                && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && h.chars().any(|c| c.is_ascii_uppercase())
        });
        if (paren || windows || mac || host) && best.is_none_or(|b| o.len() > b.len()) {
            best = Some(o);
        }
    }
    best
}

/// Where a cloud sync tool might be managing the folder.
fn cloud_folder(root: &Path) -> Option<String> {
    root.components().find_map(|c| {
        let name = c.as_os_str().to_string_lossy();
        let lower = name.to_lowercase();
        let synced = lower.starts_with("onedrive")
            || lower == "dropbox"
            || lower == "icloud drive"
            || lower == "mobile documents"
            || lower == "google drive"
            || lower == "googledrive"
            || lower == "my drive";
        synced.then(|| name.into_owned())
    })
}

/// Sorts names in a folder into the expected ones, sync copies, case
/// collisions, bad names and unknown files. `expected` decides what
/// belongs; `originals` are the names copies may be of.
fn classify(
    root: &Path,
    dir: &str,
    entries: &[(String, bool)],
    expected: impl Fn(&str, bool) -> bool,
    originals: &[&str],
    issues: &mut Vec<Issue>,
) {
    let rel = |n: &str| {
        if dir.is_empty() {
            n.to_owned()
        } else {
            format!("{dir}/{n}")
        }
    };
    let lower: BTreeMap<String, Vec<&str>> =
        entries.iter().fold(BTreeMap::new(), |mut m, (n, _)| {
            m.entry(n.to_lowercase())
                .or_insert_with(Vec::new)
                .push(n.as_str());
            m
        });
    for (name, is_dir) in entries {
        if expected(name, *is_dir) {
            continue;
        }
        let stem = name.strip_suffix(".json");
        if let Some(o) = stem.and_then(|s| copy_of(s, originals)).filter(|_| !is_dir) {
            issues.push(issue(
                "sync_conflict_copy",
                vec![rel(name), rel(&format!("{o}.json"))],
                format!(
                    "{} looks like a sync tool's copy of {}",
                    rel(name),
                    rel(&format!("{o}.json"))
                ),
            ));
            continue;
        }
        let same = &lower[&name.to_lowercase()];
        if same.len() > 1 {
            // Reported once, by the first of the colliding names.
            if same[0] == name {
                issues.push(issue(
                    "case_collision",
                    same.iter().map(|n| rel(n)).collect(),
                    format!(
                        "{} differ only in case and collide on macOS and Windows",
                        same.iter()
                            .map(|n| rel(n))
                            .collect::<Vec<_>>()
                            .join(" and ")
                    ),
                ));
            }
            continue;
        }
        let id_like = stem.unwrap_or(name);
        let looks_like_ours = if *is_dir {
            dir == "parts"
        } else {
            stem.is_some() && dir.ends_with("/features")
        };
        if looks_like_ours && Id::parse(id_like, "file").is_err() {
            issues.push(issue(
                "invalid_file_name",
                vec![rel(name)],
                format!(
                    "{} isn't a valid ID (renamed by hand?); IDs are 1 to 63 of a-z, 0-9, _ and -. Renaming changes every stable name, so it is left to you",
                    rel(name)
                ),
            ));
            continue;
        }
        let _ = root;
        issues.push(issue(
            "unknown_file",
            vec![rel(name)],
            format!("{} isn't part of a ryucad project", rel(name)),
        ));
    }
}

/// Every issue in the project at `root`, in a fixed order.
pub fn diagnose(root: &Path) -> Vec<Issue> {
    let mut issues = Vec::new();
    if let Some(name) = cloud_folder(root) {
        issues.push(issue(
            "cloud_sync_folder",
            Vec::new(),
            format!("the project is inside {name}: sync tools and Git fight over files; consider moving it"),
        ));
    }
    let mut unit = Unit::Mm;
    match read(root, MANIFEST) {
        Read::Missing => issues.push(issue(
            "missing_manifest",
            vec![MANIFEST.to_owned()],
            format!("{MANIFEST} is missing"),
        )),
        Read::Bad(i) => issues.push(i),
        Read::Json(v, canonical) => match Manifest::from_json(&v) {
            Ok(m) => {
                unit = m.default_unit;
                if !canonical {
                    issues.push(non_canonical(MANIFEST));
                }
            }
            Err(e) => issues.push(document_issue(MANIFEST, &e)),
        },
    }
    let root_names: Vec<(String, bool)> = names(root)
        .into_iter()
        .filter(|(n, d)| !d && n.starts_with("ryucad") && n.ends_with(".json") && n != MANIFEST)
        .collect();
    classify(
        root,
        "",
        &root_names,
        |_, _| false,
        &["ryucad"],
        &mut issues,
    );
    let parts = names(&root.join("parts"));
    let part_ids: Vec<&str> = parts
        .iter()
        .filter(|(n, d)| *d && Id::parse(n, "part").is_ok())
        .map(|(n, _)| n.as_str())
        .collect();
    classify(
        root,
        "parts",
        &parts,
        |n, d| d && Id::parse(n, "part").is_ok(),
        &[],
        &mut issues,
    );
    for part in part_ids {
        diagnose_part(root, part, unit, &mut issues);
    }
    issues
}

fn non_canonical(rel: &str) -> Issue {
    issue(
        "non_canonical",
        vec![rel.to_owned()],
        format!("{rel} isn't in canonical form (edited by hand?)"),
    )
}

fn diagnose_part(root: &Path, part: &str, unit: Unit, issues: &mut Vec<Issue>) {
    let dir = format!("parts/{part}");
    let index = format!("{dir}/part.json");
    let entries = names(&root.join(&dir));
    classify(
        root,
        &dir,
        &entries,
        |n, d| (n == "part.json" && !d) || (n == "features" && d),
        &["part"],
        issues,
    );
    let fdir = format!("{dir}/features");
    let files = names(&root.join(&fdir));
    let feature_ids: Vec<&str> = files
        .iter()
        .filter_map(|(n, d)| {
            (!d).then_some(n.strip_suffix(".json")?)
                .filter(|s| Id::parse(s, "feature").is_ok())
        })
        .collect();
    classify(
        root,
        &fdir,
        &files,
        |n, d| {
            !d && n
                .strip_suffix(".json")
                .is_some_and(|s| Id::parse(s, "feature").is_ok())
        },
        &feature_ids,
        issues,
    );
    for f in &feature_ids {
        let rel = format!("{fdir}/{f}.json");
        match read(root, &rel) {
            Read::Missing => {}
            Read::Bad(i) => issues.push(i),
            Read::Json(v, canonical) => match Feature::from_json(&v, unit) {
                Ok(_) if !canonical => issues.push(non_canonical(&rel)),
                Ok(_) => {}
                Err(e) => issues.push(document_issue(&rel, &e)),
            },
        }
    }
    let listed = match read(root, &index) {
        Read::Missing => {
            if !feature_ids.is_empty() {
                issues.push(issue(
                    "missing_part_index",
                    vec![index.clone()],
                    format!(
                        "{index} is missing, but {} feature file(s) remain",
                        feature_ids.len()
                    ),
                ));
            } else {
                issues.push(issue(
                    "missing_part_index",
                    vec![index.clone()],
                    format!("{index} is missing"),
                ));
            }
            return;
        }
        Read::Bad(i) => {
            issues.push(i);
            return;
        }
        Read::Json(v, canonical) => match PartFile::from_json(&v) {
            Ok(p) => {
                if !canonical {
                    issues.push(non_canonical(&index));
                }
                p.features
            }
            Err(e) => {
                issues.push(document_issue(&index, &e));
                return;
            }
        },
    };
    for f in &listed {
        if !feature_ids.contains(&f.as_str()) {
            let rel = format!("{fdir}/{f}.json");
            issues.push(issue(
                "listed_feature_missing",
                vec![rel.clone(), index.clone()],
                format!("{index} lists {f}, but {rel} is missing"),
            ));
        }
    }
    for f in &feature_ids {
        if !listed.iter().any(|l| l.as_str() == *f) {
            let rel = format!("{fdir}/{f}.json");
            issues.push(issue(
                "orphan_feature",
                vec![rel.clone(), index.clone()],
                format!("{rel} isn't listed in {index}"),
            ));
        }
    }
}

/// `doctor`: every issue, and a summary.
pub fn report(root: &Path) -> Result<Value> {
    let issues = diagnose(root);
    let errors = issues.iter().filter(|i| i.severity == "error").count();
    Ok(json!({
        "healthy": issues.is_empty(),
        "errors": errors,
        "warnings": issues.len() - errors,
        "issues": issues.iter().map(Issue::to_json).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};

    /// A healthy project: part `p` with features `a` and `b`.
    pub(crate) fn healthy() -> tempfile::TempDir {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        for id in ["a", "b"] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "sphere", "id": id, "params": {"radius": 1}}),
            )
            .unwrap();
        }
        dir
    }

    fn codes(root: &Path) -> Vec<(&'static str, Vec<String>)> {
        diagnose(root)
            .into_iter()
            .map(|i| (i.code, i.files))
            .collect()
    }

    fn w(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn a_healthy_project_has_no_issues() {
        let dir = healthy();
        assert_eq!(codes(dir.path()), []);
        assert_eq!(report(dir.path()).unwrap()["healthy"], true);
    }

    #[test]
    fn every_kind_of_damage_is_named() {
        let dir = healthy();
        let r = dir.path();
        let f = |n: &str| format!("parts/p/features/{n}.json");
        let index = "parts/p/part.json".to_owned();
        let sphere = fs::read_to_string(r.join(f("a"))).unwrap();
        // An orphan, a listed feature gone, a sync copy, a bad name, a
        // case collision, an unknown file, a non-canonical file, invalid
        // JSON, conflict markers, an invalid document, a newer format.
        w(r, &f("c"), &sphere);
        fs::remove_file(r.join(f("b"))).unwrap();
        w(r, "parts/p/features/a (1).json", &sphere);
        w(r, "parts/p/features/Hole.json", &sphere);
        w(r, "parts/p/features/notes.txt", "hi");
        w(r, &f("d"), &sphere.replace('\n', "\r\n"));
        w(r, &f("e"), "{");
        w(r, &f("g"), "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> other\n");
        w(r, &f("h"), "{}");
        w(
            r,
            &f("i"),
            &sphere.replace("\"format\": 1", "\"format\": 9"),
        );
        w(r, "parts/Q/part.json", "{}");
        w(
            r,
            "parts/q/part.json",
            r#"{"features":[],"format":1,"kind":"ryucad.part"}"#,
        );
        let got = codes(r);
        let want: Vec<(&str, Vec<String>)> = vec![
            ("case_collision", vec!["parts/Q".into(), "parts/q".into()]),
            // Byte order: upper case first.
            (
                "invalid_file_name",
                vec!["parts/p/features/Hole.json".into()],
            ),
            (
                "sync_conflict_copy",
                vec!["parts/p/features/a (1).json".into(), f("a")],
            ),
            ("unknown_file", vec!["parts/p/features/notes.txt".into()]),
            ("non_canonical", vec![f("d")]),
            ("invalid_json", vec![f("e")]),
            ("conflict_markers", vec![f("g")]),
            ("invalid_document", vec![f("h")]),
            ("format_too_new", vec![f("i")]),
            ("listed_feature_missing", vec![f("b"), index.clone()]),
            ("orphan_feature", vec![f("c"), index.clone()]),
            ("orphan_feature", vec![f("d"), index.clone()]),
            ("orphan_feature", vec![f("e"), index.clone()]),
            ("orphan_feature", vec![f("g"), index.clone()]),
            ("orphan_feature", vec![f("h"), index.clone()]),
            ("orphan_feature", vec![f("i"), index.clone()]),
            // Written by hand above, so not canonical.
            ("non_canonical", vec!["parts/q/part.json".into()]),
        ];
        assert_eq!(got, want);
        let rep = report(r).unwrap();
        assert_eq!(rep["healthy"], false);
        assert!(rep["errors"].as_u64().unwrap() > 0);
    }

    #[test]
    fn a_lost_manifest_and_index() {
        let dir = healthy();
        let r = dir.path();
        fs::remove_file(r.join(MANIFEST)).unwrap();
        fs::remove_file(r.join("parts/p/part.json")).unwrap();
        w(r, "ryucad (conflicted copy).json", "{}");
        assert_eq!(
            codes(r),
            [
                ("missing_manifest", vec![MANIFEST.to_owned()]),
                (
                    "sync_conflict_copy",
                    vec![
                        "ryucad (conflicted copy).json".to_owned(),
                        MANIFEST.to_owned()
                    ]
                ),
                ("missing_part_index", vec!["parts/p/part.json".to_owned()]),
            ]
        );
    }

    #[test]
    fn sync_copies_by_tool() {
        let o = ["boss", "boss-plate"];
        for (stem, want) in [
            ("boss (1)", Some("boss")),
            ("boss (Dana's conflicted copy 2026-10-16)", Some("boss")),
            ("boss - Copy", Some("boss")),
            ("boss - Copy (2)", Some("boss")),
            ("boss copy", Some("boss")),
            ("boss copy 3", Some("boss")),
            ("boss-DESKTOP-1A2B", Some("boss")),
            ("boss-plate (1)", Some("boss-plate")),
            ("boss-plate", None),
            ("boss-lower", None),
            ("bossy (1)", None),
        ] {
            assert_eq!(copy_of(stem, &o), want, "{stem}");
        }
    }

    #[test]
    fn a_project_in_a_synced_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("OneDrive - Seiraiyu").join("cad");
        fs::create_dir_all(&root).unwrap();
        crate::server::init(&root, None).unwrap();
        let got = codes(&root);
        assert_eq!(got, [("cloud_sync_folder", vec![])]);
        assert_eq!(diagnose(&root)[0].severity, "warning");
    }
}
