//! `doctor` (design 518): what a ryueda project folder needs that no one
//! file says. Git's setup (`gitattributes_missing`, `git_drivers_missing`),
//! paths Git left unmerged (`unmerged_path`), files not written
//! canonically (`non_canonical`), files holding a merge's conflict record
//! (`merge_conflict`), files no kind holds (`unknown_file`,
//! the loader's own warnings) and model files no footprint names
//! (`model_orphaned`: "model files a re-import no longer names stay until
//! doctor"). Each issue comes with its repairs; doctor never changes
//! anything itself.

use crate::error::{Code, Error, Result};
use crate::project::{ATTRIBUTES, Project, SELF};
use serde_json::{Value, json};
use std::path::Path;

/// One problem: its code, severity, the files involved and its repairs.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub code: Code,
    /// `error` or `warning`.
    pub severity: &'static str,
    pub files: Vec<String>,
    pub message: String,
    pub repairs: Vec<Repair>,
}

impl Issue {
    pub fn code_str(&self) -> &'static str {
        match self.code {
            Code::GitattributesMissing => "gitattributes_missing",
            Code::GitDriversMissing => "git_drivers_missing",
            Code::UnmergedPath => "unmerged_path",
            Code::NonCanonical => "non_canonical",
            Code::UnknownFile => "unknown_file",
            Code::ModelOrphaned => "model_orphaned",
            Code::MergeConflict => "merge_conflict",
            _ => "unknown",
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"code": self.code, "severity": self.severity, "files": self.files,
               "message": self.message,
               "repairs": self.repairs.iter().map(Repair::to_json).collect::<Vec<_>>()})
    }

    /// As one of `check`'s faults.
    pub fn fault(&self) -> crate::fault::Fault {
        let f = if self.severity == "error" {
            crate::fault::Fault::error(self.code, self.message.clone())
        } else {
            crate::fault::Fault::warning(self.code, self.message.clone())
        };
        f.with("files", &self.files)
    }
}

/// A way to fix an issue: exact file changes. Its ID is
/// `<code>:<action>:<first file>@<hash>`, the hash over the files' bytes
/// now, so a repair planned against files that have since changed is
/// refused.
#[derive(Clone, Debug, PartialEq)]
pub struct Repair {
    pub id: String,
    pub summary: String,
    pub changes: Vec<Change>,
}

/// A project file written (these bytes) or, with `None`, deleted; or, with
/// the path [`GIT_INSTALL`], the clone set up as `ryueda git install` does
/// (Git's config, not project files: not logged or undone).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    pub content: Option<Vec<u8>>,
}

/// The change that runs `ryueda git install` in the project's clone.
pub const GIT_INSTALL: &str = "ryueda git install";

impl Repair {
    fn new(
        code: &str,
        action: &str,
        root: &Path,
        files: &[&str],
        summary: String,
        changes: Vec<Change>,
    ) -> Repair {
        let mut h = blake3::Hasher::new();
        for f in files {
            h.update(f.as_bytes());
            h.update(&[0]);
            h.update(&std::fs::read(crate::session::path_of(root, f)).unwrap_or_default());
        }
        let first = files.first().copied().unwrap_or_default();
        Repair {
            id: format!("{code}:{action}:{first}@{}", &h.finalize().to_hex()[..8]),
            summary,
            changes,
        }
    }

    pub fn to_json(&self) -> Value {
        let changes: Vec<Value> = self.changes.iter().map(|c| match &c.content {
            _ if c.path == GIT_INSTALL => json!({"action": "git_install"}),
            Some(b) => json!({"path": c.path, "action": "write", "content": String::from_utf8_lossy(b)}),
            None => json!({"path": c.path, "action": "delete"}),
        }).collect();
        json!({"id": self.id, "summary": self.summary, "changes": changes})
    }
}

fn issue(
    code: Code,
    severity: &'static str,
    files: Vec<String>,
    message: String,
    repairs: Vec<Repair>,
) -> Issue {
    Issue {
        code,
        severity,
        files,
        message,
        repairs,
    }
}

/// Every issue of the project at `root` (loaded as `p`), sorted by code
/// then files.
pub fn diagnose(root: &Path, p: &Project) -> Vec<Issue> {
    let mut out = Vec::new();
    git_issues(root, &mut out);
    canonical_issues(root, &mut out);
    conflict_issues(root, &mut out);
    for f in p
        .faults()
        .iter()
        .filter(|f| f.error.code == Code::UnknownFile)
    {
        let path = f.error.details["path"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        out.push(issue(
            Code::UnknownFile,
            "warning",
            vec![path],
            f.error.message.clone(),
            Vec::new(),
        ));
    }
    orphaned_models(root, p, &mut out);
    out.sort_by(|a, b| {
        a.code_str()
            .cmp(b.code_str())
            .then_with(|| a.files.cmp(&b.files))
    });
    out
}

fn git_issues(root: &Path, out: &mut Vec<Issue>) {
    let Some(setup) = crate::drivers::setup(root) else {
        return;
    };
    if let Ok(Some(text)) = crate::drivers::attributes_wanted(root) {
        let missing = crate::drivers::missing(
            &std::fs::read_to_string(root.join(ATTRIBUTES)).unwrap_or_default(),
        );
        let quoted: Vec<String> = missing.iter().map(|l| format!("\"{l}\"")).collect();
        out.push(issue(
            Code::GitattributesMissing,
            "warning",
            vec![ATTRIBUTES.to_owned()],
            format!(
                "{ATTRIBUTES} lacks {}: Git won't read the project's files through ryueda",
                quoted.join(", ")
            ),
            vec![Repair::new(
                "gitattributes_missing",
                "write",
                root,
                &[ATTRIBUTES],
                format!("add ryueda's lines to {ATTRIBUTES}, keeping the rest"),
                vec![Change {
                    path: ATTRIBUTES.to_owned(),
                    content: Some(text.into_bytes()),
                }],
            )],
        ));
    }
    if !setup.missing.is_empty() {
        out.push(issue(
            Code::GitDriversMissing,
            "warning",
            Vec::new(),
            format!(
                "Git lacks {}: run `ryueda git install`",
                setup.missing.join(", ")
            ),
            vec![Repair::new(
                "git_drivers_missing",
                "git_install",
                root,
                &[],
                "run ryueda git install in this clone".into(),
                vec![Change {
                    path: GIT_INSTALL.to_owned(),
                    content: None,
                }],
            )],
        ));
    }
    if let Ok(Some(repo)) = crate::git::Repo::open(root)
        && let Ok(unmerged) = repo.unmerged()
    {
        for path in unmerged.keys() {
            out.push(issue(
                Code::UnmergedPath,
                "error",
                vec![path.clone()],
                format!("Git left {path} unmerged: settle it and `git add` it"),
                Vec::new(),
            ));
        }
    }
}

/// The project's own JSON files holding a conflict record: a merge left
/// unfinished (4.C's `merge status` settles them).
fn conflict_issues(root: &Path, out: &mut Vec<Issue>) {
    let Ok(files) = crate::session::current(root) else {
        return;
    };
    for (path, bytes) in files
        .iter()
        .filter(|(p, _)| !p.starts_with("../") && p.ends_with(".json"))
    {
        let Some(v) = std::str::from_utf8(bytes)
            .ok()
            .and_then(|t| crate::canonical::parse(t).ok())
        else {
            continue;
        };
        if matches!(v.get(crate::doc::CONFLICTS), Some(Value::Object(_))) {
            out.push(issue(
                Code::MergeConflict,
                "error",
                vec![path.clone()],
                format!("{path} has unresolved merge conflicts: settle them, then commit"),
                Vec::new(),
            ));
        }
    }
}

/// The project's own JSON files (never an alias's) not written as
/// [`crate::canonical::to_string`] writes them.
fn canonical_issues(root: &Path, out: &mut Vec<Issue>) {
    let Ok(files) = crate::session::current(root) else {
        return;
    };
    for (path, bytes) in files
        .iter()
        .filter(|(p, _)| !p.starts_with("../") && p.ends_with(".json"))
    {
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let Ok(v) = crate::canonical::parse(text) else {
            continue;
        };
        let canon = crate::canonical::to_string(&v);
        if canon != text {
            out.push(issue(
                Code::NonCanonical,
                "warning",
                vec![path.clone()],
                format!("{path} isn't in canonical form (edited by hand?)"),
                vec![Repair::new(
                    "non_canonical",
                    "write",
                    root,
                    &[path],
                    format!("write {path} canonically"),
                    vec![Change {
                        path: path.clone(),
                        content: Some(canon.into_bytes()),
                    }],
                )],
            ));
        }
    }
}

/// Files under the project's own `library/models/` no footprint of its
/// library names.
fn orphaned_models(root: &Path, p: &Project, out: &mut Vec<Issue>) {
    let Ok(lib) = p.library(SELF) else { return };
    let named: std::collections::BTreeSet<&str> = lib
        .footprints
        .values()
        .filter_map(|f| f.item.as_ref().ok())
        .flat_map(|f| f.models.iter())
        .map(|m| match &m.model {
            crate::footprint::Model::Step(s) => s.path.as_str(),
        })
        .collect();
    let dir = crate::source::join(crate::library::DIR, crate::library::MODELS);
    let Ok(entries) = std::fs::read_dir(crate::session::path_of(root, &dir)) else {
        return;
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| crate::source::join(&dir, &e.file_name().to_string_lossy()))
        .filter(|rel| !named.contains(rel.as_str()))
        .collect();
    found.sort();
    for rel in found {
        out.push(issue(
            Code::ModelOrphaned,
            "warning",
            vec![rel.clone()],
            format!("no footprint names {rel}: an import no longer does"),
            vec![Repair::new(
                "model_orphaned",
                "remove",
                root,
                &[&rel],
                format!("remove {rel}"),
                vec![Change {
                    path: rel.clone(),
                    content: None,
                }],
            )],
        ));
    }
}

/// `doctor`: every issue with its repairs. Writes nothing.
pub fn report(root: &Path, p: &Project) -> Value {
    let issues = diagnose(root, p);
    let errors = issues.iter().filter(|i| i.severity == "error").count();
    json!({
        "healthy": issues.is_empty(),
        "errors": errors,
        "warnings": issues.len() - errors,
        "issues": issues.iter().map(Issue::to_json).collect::<Vec<_>>(),
    })
}

/// The repairs `ids` names, planned now, in order; `repair_conflict` for
/// one no fresh diagnosis offers (the files changed, or it never was).
pub fn planned(root: &Path, p: &Project, ids: &[String]) -> Result<Vec<Repair>> {
    let all: Vec<Repair> = diagnose(root, p)
        .into_iter()
        .flat_map(|i| i.repairs)
        .collect();
    ids.iter()
        .map(|id| {
            all.iter().find(|r| r.id == *id).cloned().ok_or_else(|| {
                Error::new(Code::RepairConflict,
                    format!("no repair {id} now: the files changed, or it was never offered; run doctor again"))
                    .with("repair", id)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::{library_in, manifest};

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code_str()).collect()
    }

    #[test]
    fn outside_a_repository_there_is_no_git_to_judge() {
        let dir = tempfile::tempdir().unwrap();
        manifest(dir.path(), ProjectKind::Library, &[]);
        let p = Project::load(dir.path()).unwrap();
        // A manifest written by ryueda, no attributes file, no repository.
        assert_eq!(codes(&diagnose(dir.path(), &p)), Vec::<&str>::new());
    }

    #[test]
    fn a_hand_written_file_and_an_orphaned_model() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        manifest(r, ProjectKind::Library, &[]);
        library_in(r);
        let sym = std::fs::read_dir(r.join("library/symbols"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = std::fs::read_to_string(&sym).unwrap();
        let one_line: serde_json::Value = serde_json::from_str(&text).unwrap();
        std::fs::write(&sym, one_line.to_string()).unwrap();
        std::fs::create_dir_all(r.join("library/models")).unwrap();
        std::fs::write(r.join("library/models/old.step"), b"ISO-10303-21;").unwrap();
        let p = Project::load(r).unwrap();
        let issues = diagnose(r, &p);
        let rel = format!(
            "library/symbols/{}",
            sym.file_name().unwrap().to_string_lossy()
        );
        let canon = issues
            .iter()
            .find(|i| i.code == Code::NonCanonical && i.files == [rel.clone()])
            .unwrap();
        let canon = &canon.repairs[0];
        assert!(
            canon.id.starts_with(&format!("non_canonical:write:{rel}@")),
            "{}",
            canon.id
        );
        // What the repair writes is the file as ryueda writes it.
        let want = crate::canonical::to_string(&crate::canonical::parse(&text).unwrap());
        assert_eq!(canon.changes[0].content.as_deref(), Some(want.as_bytes()));
        let orphan = issues
            .iter()
            .find(|i| i.code == Code::ModelOrphaned)
            .unwrap();
        assert_eq!(orphan.files, ["library/models/old.step"]);
        let orphan = &orphan.repairs[0];
        assert!(
            orphan
                .id
                .starts_with("model_orphaned:remove:library/models/old.step@"),
            "{}",
            orphan.id
        );
        assert_eq!(orphan.changes[0].content, None);
    }

    #[test]
    fn in_a_repository_attributes_and_drivers() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        crate::git::tests::git(r, &["init", "-q"]);
        manifest(r, ProjectKind::Design, &[]);
        let p = Project::load(r).unwrap();
        let issues = diagnose(r, &p);
        assert_eq!(
            codes(&issues),
            ["git_drivers_missing", "gitattributes_missing"]
        );
        assert_eq!(issues[0].repairs[0].changes[0].path, GIT_INSTALL);
    }

    #[test]
    fn a_path_git_left_unmerged_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let git = |a: &[&str]| crate::git::tests::git(r, a);
        git(&["init", "-q"]);
        manifest(r, ProjectKind::Design, &[]);
        std::fs::write(
            r.join("lint.json"),
            "{\"format\": 1, \"kind\": \"ryueda.lint\"}\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["checkout", "-q", "-b", "theirs"]);
        std::fs::write(
            r.join("lint.json"),
            "{\"format\": 1, \"kind\": \"ryueda.lint\", \"weights\": {}}\n",
        )
        .unwrap();
        git(&["commit", "-q", "-am", "theirs"]);
        git(&["checkout", "-q", "main"]);
        std::fs::write(
            r.join("lint.json"),
            "{\"format\": 1, \"kind\": \"ryueda.lint\", \"severity\": {}}\n",
        )
        .unwrap();
        git(&["commit", "-q", "-am", "ours"]);
        let merged = crate::git::tests::git_command()
            .args(["merge", "theirs"])
            .current_dir(r)
            .output()
            .unwrap();
        assert!(!merged.status.success(), "the merge conflicts");
        let p = Project::load(r).unwrap();
        let issues = diagnose(r, &p);
        let u = issues
            .iter()
            .find(|i| i.code == Code::UnmergedPath)
            .unwrap();
        assert_eq!(
            (u.files.as_slice(), u.severity),
            (&["lint.json".to_owned()][..], "error")
        );
    }
}
