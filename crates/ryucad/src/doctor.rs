//! Diagnosing and repairing a project folder. A folder can break in ways a
//! single file can't: a lost manifest, a feature file nobody lists, Git
//! conflict markers, a sync tool's conflicted copy. `doctor` reports each
//! with a stable code, and never changes anything itself.
//!
//! It reads the folder directly, not through [`crate::doc::project`], so
//! it works on a project that can't be opened. Only a sketch that reads
//! other features is judged by loading the project and rebuilding its
//! part, when the project loads.

use crate::doc::canonical;
use crate::doc::files::{Feature, Manifest, ParametersFile, PartFile, PartKind};
use crate::doc::id::Id;
use crate::doc::id::ProjectId;
use crate::doc::project::{IMPORTS, MANIFEST, PDM_CONFIG, Project, write_atomic};
use crate::doc::source::Folder;
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::git::Repo;
use crate::rebuild::PartResult;
use crate::session::Session;
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
pub const CODES: [(&str, &str); 34] = [
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
    ("sketch_stale", "warning"),
    ("assembly_stale", "warning"),
    ("gitattributes_missing", "warning"),
    ("git_drivers_missing", "warning"),
    ("merge_conflict", "error"),
    ("git_hook_missing", "warning"),
    ("git_hook_foreign", "warning"),
    ("property_unknown", "error"),
    ("property_invalid", "error"),
    ("part_number_invalid", "error"),
    ("part_number_duplicate", "error"),
    ("project_id_missing", "warning"),
    ("import_file_missing", "error"),
    ("import_file_changed", "error"),
    ("import_unused", "warning"),
    ("import_large", "warning"),
    ("bom_item_missing", "warning"),
    ("bom_item_duplicate", "error"),
    ("bom_item_unused", "warning"),
    ("material_unknown", "warning"),
];

/// The size past which an imported file is `import_large`: Git keeps
/// every version of it whole.
pub const LARGE_IMPORT: u64 = 100 * 1024 * 1024;

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
        Ok(v) if crate::doc::project::conflicted(&v, rel).is_some() => Read::Bad(issue(
            "merge_conflict",
            vec![rel.to_owned()],
            format!(
                "{rel} has unresolved merge conflicts: see `ryucad merge status`, then `ryucad merge resolve`"
            ),
        )),
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

/// Where doctor's rebuilds come from: the daemon's session, which is
/// current and keeps its results, else a session opened on the folder the
/// first time one is needed and shared by every check after (issue #125:
/// a daemon's doctor rebuilt each assembly cold, blocking the session).
pub(crate) struct Builds<'a> {
    root: &'a Path,
    live: Option<&'a mut Session>,
    opened: Option<Option<Session>>,
}

impl<'a> Builds<'a> {
    /// Rebuilds from a session opened on `root` when one is needed.
    pub(crate) fn cold(root: &'a Path) -> Self {
        Self {
            root,
            live: None,
            opened: None,
        }
    }

    /// Rebuilds from `session`, whose files must be current.
    pub(crate) fn live(root: &'a Path, session: &'a mut Session) -> Self {
        Self {
            root,
            live: Some(session),
            opened: None,
        }
    }

    /// The session, `None` when the project doesn't load.
    fn session(&mut self) -> Option<&mut Session> {
        if let Some(s) = self.live.as_deref_mut() {
            return Some(s);
        }
        let root = self.root;
        self.opened
            .get_or_insert_with(|| Session::open(root).ok())
            .as_mut()
    }

    /// Part `part` as loaded and its rebuild, if the project loads.
    fn rebuilt(&mut self, part: &str) -> Option<(crate::doc::project::Part, PartResult)> {
        let id = Id::parse(part, "part").ok()?;
        let s = self.session()?;
        let p = s.project().parts.get(&id)?.clone();
        let r = s.rebuilt(&id).ok()?.clone();
        Some((p, r))
    }
}

/// Every issue in the project at `root`, in a fixed order.
pub fn diagnose(root: &Path) -> Vec<Issue> {
    diagnose_in(&mut Builds::cold(root))
}

/// Every issue, rebuilding through `builds`.
fn diagnose_in(builds: &mut Builds) -> Vec<Issue> {
    let root = builds.root;
    let mut issues = Vec::new();
    if let Some(name) = cloud_folder(root) {
        issues.push(issue(
            "cloud_sync_folder",
            Vec::new(),
            format!("the project is inside {name}: sync tools and Git fight over files; consider moving it"),
        ));
    }
    let mut unit = Unit::Mm;
    let mut manifest = None;
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
                manifest = Some(m);
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
    for part in &part_ids {
        diagnose_part(builds, part, unit, &mut issues);
    }
    diagnose_assemblies(builds, &part_ids, &mut issues);
    diagnose_items(&mut issues, root);
    diagnose_imports(root, &part_ids, unit, &mut issues);
    diagnose_pdm(root, manifest.as_ref(), &part_ids, &mut issues);
    diagnose_materials(root, &part_ids, &mut issues);
    diagnose_releases(root, &mut issues);
    diagnose_git(root, &mut issues);
    issues
}

/// The files under `imports/` and the import features that read them: a
/// name that isn't `<id>.step` is unknown; a listed import whose file is
/// missing, or isn't the bytes it was written against, is an error; a
/// file no listed import reads is unused; one past [`LARGE_IMPORT`] is
/// large.
fn diagnose_imports(root: &Path, parts: &[&str], unit: Unit, issues: &mut Vec<Issue>) {
    use crate::doc::import::is_import_path;
    let entries = names(&root.join(IMPORTS));
    classify(
        root,
        IMPORTS,
        &entries,
        |n, d| !d && is_import_path(&format!("{IMPORTS}/{n}")),
        &[],
        issues,
    );
    // Each file's readers: the feature files, with the hash they name.
    let mut readers: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for part in parts {
        let Some(list) = listed(root, part) else {
            continue;
        };
        for fid in &list.features {
            let rel = format!("parts/{part}/features/{fid}.json");
            let Read::Json(v, _) = read(root, &rel) else {
                continue;
            };
            if v.get("type").and_then(Value::as_str) != Some("import") {
                continue;
            }
            if let Ok(Feature::Import(i)) = Feature::from_json(&v, unit, &Env::empty()) {
                readers
                    .entry(i.file.path)
                    .or_default()
                    .push((rel, i.file.blake3));
            }
        }
    }
    for (path, by) in &readers {
        let features: Vec<String> = by.iter().map(|(f, _)| f.clone()).collect();
        let Ok(bytes) = fs::read(root.join(path)) else {
            issues.push(issue(
                "import_file_missing",
                [features.clone(), vec![path.clone()]].concat(),
                format!(
                    "{path} is missing, and {} import{} it: restore it from Git or import the file again",
                    features.join(", "),
                    if features.len() == 1 { "s" } else { "" }
                ),
            ));
            continue;
        };
        let now = blake3::hash(&bytes).to_hex().to_string();
        let changed: Vec<String> = by
            .iter()
            .filter(|(_, h)| *h != now)
            .map(|(f, _)| f.clone())
            .collect();
        if !changed.is_empty() {
            issues.push(issue(
                "import_file_changed",
                [changed.clone(), vec![path.clone()]].concat(),
                format!(
                    "{path} isn't the file {} imported (replaced by hand?): import it again to take its new bytes, or restore it from Git",
                    changed.join(", ")
                ),
            ));
        }
    }
    for (name, is_dir) in &entries {
        let path = format!("{IMPORTS}/{name}");
        if *is_dir || !is_import_path(&path) {
            continue;
        }
        if !readers.contains_key(&path) {
            issues.push(issue(
                "import_unused",
                vec![path.clone()],
                format!("no import feature reads {path}"),
            ));
        }
        let size = fs::metadata(root.join(&path)).map_or(0, |m| m.len());
        if size > LARGE_IMPORT {
            issues.push(issue(
                "import_large",
                vec![path.clone()],
                format!(
                    "{path} is {} MB: Git keeps every version of it whole, in every clone",
                    size / (1024 * 1024)
                ),
            ));
        }
    }
}

/// Each assembly whose stored poses don't hold where its mates do, with
/// the instance files its repair writes.
fn diagnose_assemblies(builds: &mut Builds, parts: &[&str], issues: &mut Vec<Issue>) {
    let root = builds.root;
    let assembly = |part: &str| {
        matches!(read(root, &format!("parts/{part}/part.json")),
            Read::Json(v, _) if v["part_kind"] == "assembly")
    };
    let assemblies: Vec<&str> = parts.iter().copied().filter(|p| assembly(p)).collect();
    if assemblies.is_empty() {
        return;
    }
    let Some(session) = builds.session() else {
        return;
    };
    for part in assemblies {
        let Some(writes) = stale_assembly(session, part) else {
            continue;
        };
        let files: Vec<String> = writes.into_iter().map(|(rel, _)| rel).collect();
        issues.push(issue(
            "assembly_stale",
            files,
            format!("parts/{part}: its stored poses don't satisfy its mates"),
        ));
    }
}

/// Each assembly's item numbers, in a project that loads: placed parts
/// with none, numbers shared, numbers of parts it no longer places
/// ([`ryupdm::items::faults`]).
fn diagnose_items(issues: &mut Vec<Issue>, root: &Path) {
    let Ok(project) = Project::load(root) else {
        return;
    };
    for (id, part) in &project.parts {
        for e in ryupdm::items::faults(id, part) {
            let code = match e.code {
                Code::BomItemMissing => "bom_item_missing",
                Code::BomItemDuplicate => "bom_item_duplicate",
                _ => "bom_item_unused",
            };
            issues.push(issue(
                code,
                vec![format!("parts/{id}/part.json")],
                e.message,
            ));
        }
    }
}

/// The instance files of assembly `part` as they would be with its poses
/// solved, when its stored poses are stale and its mates hold where they
/// are solved; `None` otherwise.
fn stale_assembly(session: &mut Session, part: &str) -> Option<Vec<(String, Vec<u8>)>> {
    let id = Id::parse(part, "part").ok()?;
    let r = session.rebuilt(&id).ok()?.clone();
    let solve = r.solve.as_ref()?;
    if !solve.stale() || !solve.report.converged {
        return None;
    }
    let p = session.project().part(&id).ok()?.clone();
    let moved = crate::mate::Moved {
        result: r,
        held: false,
        moves: Vec::new(),
    };
    Some(
        moved
            .writes(&p)
            .into_iter()
            .map(|(fid, f)| {
                (
                    format!("parts/{part}/features/{fid}.json"),
                    canonical_bytes(&f.to_json()),
                )
            })
            .collect(),
    )
}

/// The material library: a file that doesn't read, and, when it reads,
/// each part whose PDM `material` property names no material of it
/// (`material_unknown`: its mass properties would have no density).
fn diagnose_materials(root: &Path, parts: &[&str], issues: &mut Vec<Issue>) {
    use crate::doc::materials::{MATERIALS, Materials};
    let library = match read(root, MATERIALS) {
        Read::Missing => return,
        Read::Bad(i) => return issues.push(i),
        Read::Json(v, canonical) => match Materials::from_json(&v) {
            Ok(m) => {
                if !canonical {
                    issues.push(non_canonical(MATERIALS));
                }
                m
            }
            Err(e) => return issues.push(document_issue(MATERIALS, &e)),
        },
    };
    for part in parts {
        let Ok(id) = Id::parse(part, "part") else {
            continue;
        };
        let Ok(props) = ryupdm::props::read(&Folder(root), &id) else {
            continue;
        };
        if let Some(name) = props.values.get("material").and_then(Value::as_str)
            && !library.materials.contains_key(name)
        {
            issues.push(issue(
                "material_unknown",
                vec![format!("parts/{part}/properties.json"), MATERIALS.to_owned()],
                format!(
                    "part {part}'s material {name:?} isn't in {MATERIALS}: add it with `ryucad materials set`, or change the part's material"
                ),
            ));
        }
    }
}

/// ryupdm's files: its settings and each part's properties read as they
/// should, the properties fit the settings, and a project that uses PDM
/// has its ID.
fn diagnose_pdm(root: &Path, manifest: Option<&Manifest>, parts: &[&str], issues: &mut Vec<Issue>) {
    let config_ok = match read(root, PDM_CONFIG) {
        Read::Missing => true,
        Read::Bad(i) => {
            issues.push(i);
            false
        }
        Read::Json(v, canonical) => match ryupdm::config::Config::from_json(&v) {
            Ok(_) => {
                if !canonical {
                    issues.push(non_canonical(PDM_CONFIG));
                }
                true
            }
            Err(e) => {
                issues.push(document_issue(PDM_CONFIG, &e));
                false
            }
        },
    };
    for part in parts {
        let rel = format!("parts/{part}/properties.json");
        match read(root, &rel) {
            Read::Missing => {}
            Read::Bad(i) => issues.push(i),
            Read::Json(v, canonical) => match ryupdm::props::Properties::from_json(&v) {
                Ok(_) if !canonical => issues.push(non_canonical(&rel)),
                Ok(_) => {}
                Err(e) => issues.push(document_issue(&rel, &e)),
            },
        }
    }
    // Properties are judged against readable settings, in a project that
    // loads; a file that can't be read is reported above instead.
    if let (true, Ok(project)) = (config_ok, Project::load(root)) {
        let pdm = ryupdm::props::Pdm::read(&Folder(root), &project);
        for (id, p) in &pdm.parts {
            for f in &p.faults {
                let code = match f.code {
                    Code::PropertyUnknown => "property_unknown",
                    Code::PropertyInvalid => "property_invalid",
                    Code::PartNumberInvalid => "part_number_invalid",
                    _ => "part_number_duplicate",
                };
                let mut files = vec![format!("parts/{id}/properties.json")];
                if let Some(others) = f.details["parts"].as_array() {
                    files.extend(
                        others
                            .iter()
                            .filter_map(Value::as_str)
                            .map(|o| format!("parts/{o}/properties.json")),
                    );
                }
                issues.push(issue(code, files, f.message.clone()));
            }
        }
    }
    let uses_pdm = root.join(PDM_CONFIG).is_file() || crate::pdm::released(root);
    if uses_pdm && manifest.is_some_and(|m| m.project.is_none()) {
        let mut files = vec![MANIFEST.to_owned()];
        if root.join(PDM_CONFIG).is_file() {
            files.push(PDM_CONFIG.to_owned());
        }
        issues.push(issue(
            "project_id_missing",
            files,
            format!("the project uses ryupdm but {MANIFEST} has no project ID"),
        ));
    }
}

/// ryupdm's release records: `releases/` holds a folder per part ID and
/// in it only records, each of which reads as a record of the part and
/// label its path gives. Whether they agree with the parts and history is
/// `release check`'s to say.
fn diagnose_releases(root: &Path, issues: &mut Vec<Issue>) {
    use crate::doc::project::RELEASES;
    use ryupdm::record::parse_name;
    let dirs = names(&root.join(RELEASES));
    classify(
        root,
        RELEASES,
        &dirs,
        |n, d| d && Id::parse(n, "part").is_ok(),
        &[],
        issues,
    );
    for (dir, _) in dirs
        .iter()
        .filter(|(n, d)| *d && Id::parse(n, "part").is_ok())
    {
        let at = format!("{RELEASES}/{dir}");
        let files = names(&root.join(RELEASES).join(dir));
        let labels: Vec<String> = files
            .iter()
            .filter_map(|(n, d)| (!d).then(|| parse_name(n)).flatten())
            .map(|(l, k)| match k {
                ryupdm::record::Kind::Release => l,
                ryupdm::record::Kind::Obsolete => format!("{l}.obsolete"),
            })
            .collect();
        let originals: Vec<&str> = labels.iter().map(String::as_str).collect();
        classify(
            root,
            &at,
            &files,
            |n, d| !d && parse_name(n).is_some(),
            &originals,
            issues,
        );
    }
    let Ok(records) = ryupdm::record::Records::read(&Folder(root)) else {
        return;
    };
    for e in &records.entries {
        match read(root, &e.path) {
            Read::Missing => {}
            Read::Bad(i) => issues.push(i),
            Read::Json(_, canonical) => match &e.record {
                Err(err) => issues.push(document_issue(&e.path, err)),
                Ok(_) if !canonical => issues.push(non_canonical(&e.path)),
                Ok(_) => {}
            },
        }
    }
}

/// The project ID a release record names, if any does: the first in path
/// order.
fn recorded_project(root: &Path) -> Option<ProjectId> {
    let mut paths: Vec<std::path::PathBuf> = fs::read_dir(root.join(crate::pdm::RELEASES))
        .ok()?
        .flatten()
        .flat_map(|d| fs::read_dir(d.path()).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths.iter().find_map(|p| {
        let v = canonical::parse(&fs::read_to_string(p).ok()?).ok()?;
        ProjectId::parse(v["project"].as_str()?).ok()
    })
}

/// An ID for a project that lost its own and has no record naming one:
/// drawn from its folder and the files involved, so the repair doctor
/// reports is the repair it makes.
fn derived_project(root: &Path, files: &[String]) -> ProjectId {
    let mut h = blake3::Hasher::new();
    h.update(b"ryupdm.project\0");
    h.update(root.to_string_lossy().as_bytes());
    for f in files {
        h.update(&[0]);
        h.update(&fs::read(root.join(f)).unwrap_or_default());
    }
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&h.finalize().as_bytes()[..16]);
    ProjectId::from_bytes(bytes)
}

/// In a repository: what lets Git read and merge the project's files
/// through ryucad, and refuse a merge commit while the merge is unresolved.
fn diagnose_git(root: &Path, issues: &mut Vec<Issue>) {
    use crate::drivers::{ATTRIBUTES, Hook};
    let Some(setup) = crate::drivers::setup(root) else {
        return;
    };
    let text = fs::read_to_string(root.join(ATTRIBUTES));
    let missing = crate::drivers::missing_for(
        text.as_deref().unwrap_or_default(),
        crate::drivers::has_imports(root),
    );
    if !missing.is_empty() {
        // Each line whole and quoted: a line's pattern may be a file name
        // (`.gitattributes text eol=lf`), which read bare looked like the
        // message naming the file twice (issue #163).
        let what = if text.is_err() {
            format!("there is no {ATTRIBUTES}")
        } else {
            let quoted: Vec<String> = missing.iter().map(|l| format!("\"{l}\"")).collect();
            match quoted.split_last() {
                Some((last, [])) => format!("{ATTRIBUTES} lacks the line {last}"),
                Some((last, rest)) => {
                    format!(
                        "{ATTRIBUTES} lacks the lines {} and {last}",
                        rest.join(", ")
                    )
                }
                None => unreachable!(),
            }
        };
        issues.push(issue(
            "gitattributes_missing",
            vec![ATTRIBUTES.to_owned()],
            format!("{what}: Git won't read or merge the project's files through ryucad"),
        ));
    }
    if !setup.missing.is_empty() {
        let renames = setup.missing.contains(&crate::drivers::RENAMES);
        let drivers: Vec<&str> = setup
            .missing
            .iter()
            .copied()
            .filter(|k| *k != crate::drivers::RENAMES)
            .collect();
        let mut why = Vec::new();
        if !drivers.is_empty() {
            why.push(format!("Git has no {}", drivers.join(" or ")));
        }
        if renames {
            why.push(format!(
                "{} isn't false, so Git can take a deleted feature and an added one for a rename and merge them past ryucad",
                crate::drivers::RENAMES
            ));
        }
        issues.push(issue(
            "git_drivers_missing",
            Vec::new(),
            format!("{}: run `ryucad git install`", why.join("; ")),
        ));
    }
    let line = crate::drivers::hook_line().unwrap_or_default();
    for (name, hook) in setup.hooks {
        match hook {
            Hook::Installed(_) => {}
            Hook::Missing(p) => issues.push(issue(
                "git_hook_missing",
                Vec::new(),
                format!(
                    "this clone has no {name} hook ({}): a merge can be committed while it leaves the model broken; run `ryucad git install`",
                    p.display()
                ),
            )),
            Hook::HooksPath(p) => issues.push(issue(
                "git_hook_missing",
                Vec::new(),
                format!(
                    "core.hooksPath is set and its {name} hook ({}) doesn't run ryucad's merge check; add this line to it: {line}",
                    p.display()
                ),
            )),
            Hook::Foreign(p) => issues.push(issue(
                "git_hook_foreign",
                Vec::new(),
                format!(
                    "the {name} hook ({}) is another tool's, so ryucad left it alone; add this line to it: {line}",
                    p.display()
                ),
            )),
        }
    }
}

fn non_canonical(rel: &str) -> Issue {
    issue(
        "non_canonical",
        vec![rel.to_owned()],
        format!("{rel} isn't in canonical form (edited by hand?)"),
    )
}

fn diagnose_part(builds: &mut Builds, part: &str, unit: Unit, issues: &mut Vec<Issue>) {
    let root = builds.root;
    let dir = format!("parts/{part}");
    let index = format!("{dir}/part.json");
    let entries = names(&root.join(&dir));
    classify(
        root,
        &dir,
        &entries,
        |n, d| {
            (n == "part.json" && !d)
                || (n == "parameters.json" && !d)
                || (n == "properties.json" && !d)
                || (n == "features" && d)
        },
        &["part", "parameters", "properties"],
        issues,
    );
    let params = format!("{dir}/parameters.json");
    // Features are checked against the parameters; when those can't be
    // read, that is reported once, not again for every feature.
    let env = match read(root, &params) {
        Read::Missing => Env::empty(),
        Read::Bad(i) => {
            issues.push(i);
            Env::broken(Error::new(Code::InvalidJson, "unreadable").with("parameters", &params))
        }
        Read::Json(v, canonical) => match ParametersFile::from_json(&v) {
            Ok(p) => {
                if !canonical {
                    issues.push(non_canonical(&params));
                }
                p.env()
            }
            Err(e) => {
                issues.push(document_issue(&params, &e));
                Env::broken(e.with("parameters", &params))
            }
        },
    };
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
    let mut in_part: Option<BTreeMap<String, Vec<String>>> = None;
    for f in &feature_ids {
        let rel = format!("{fdir}/{f}.json");
        match read(root, &rel) {
            Read::Missing => {}
            Read::Bad(i) => issues.push(i),
            Read::Json(v, canonical) => match Feature::from_json(&v, unit, &env) {
                Ok(feature) => {
                    if !canonical {
                        issues.push(non_canonical(&rel));
                    }
                    // A sketch that reads other features is judged in its
                    // part, rebuilt once for all of them.
                    let reads = feature.sketch().is_some_and(|s| !s.references().is_empty());
                    let stale = if reads {
                        let in_part = in_part.get_or_insert_with(|| stale_in_part(builds, part));
                        in_part.get(*f).cloned()
                    } else {
                        solved_sketch(f, &feature).map(|_| Vec::new())
                    };
                    if let Some(inputs) = stale {
                        // Its parameters and what it reads are part of why,
                        // and of the repair's basis.
                        let mut files = vec![rel.clone()];
                        if root.join(&params).is_file() {
                            files.push(params.clone());
                        }
                        files.extend(inputs);
                        issues.push(issue(
                            "sketch_stale",
                            files,
                            format!("{rel}: its stored positions don't satisfy its constraints"),
                        ));
                    }
                }
                Err(e) if e.details.get("parameters").is_some() => {}
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

/// A sketch whose stored positions don't hold, solved; `None` for any
/// other feature, and for a sketch that is current or can't be solved.
fn solved_sketch(id: &str, f: &Feature) -> Option<Feature> {
    let id = Id::parse(id, "feature").ok()?;
    // A sketch that reads other features is solved in its part.
    let s = f.sketch()?;
    let ctx = crate::sketch::Context::standalone(s)?.ok()?;
    let solved = crate::sketch::solve_in(&id, s, &ctx).ok()?;
    solved.stale.then_some(Feature::Sketch(solved.sketch))
}

/// The stale sketches of `part` that read other features, each with the
/// files of the features it reads, directly or not. Empty when the
/// project can't be loaded.
fn stale_in_part(builds: &mut Builds, part: &str) -> BTreeMap<String, Vec<String>> {
    let Some((p, r)) = builds.rebuilt(part) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (id, o) in &r.features {
        let Some(s) = o.as_ref().ok().and_then(|o| o.sketch()) else {
            continue;
        };
        if !s.stale || s.sketch.references().is_empty() {
            continue;
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut todo = vec![id.clone()];
        while let Some(f) = todo.pop() {
            let Some(Ok(feature)) = p.features.get(&f) else {
                continue;
            };
            for i in crate::graph::inputs(feature) {
                if seen.insert(i.clone()) {
                    todo.push(i.clone());
                }
            }
        }
        let files = seen
            .iter()
            .map(|i| format!("parts/{part}/features/{i}.json"))
            .collect();
        out.insert(id.to_string(), files);
    }
    out
}

/// A feature file as it would be with its sketch solved, read with the
/// project's unit and the part's parameters as they are now; a sketch that
/// reads other features, solved in its part.
fn solved_file(builds: &mut Builds, rel: &str) -> Option<Vec<u8>> {
    let root = builds.root;
    let unit = match read(root, MANIFEST) {
        Read::Json(v, _) => Manifest::from_json(&v).ok()?.default_unit,
        _ => Unit::Mm,
    };
    let env = match read(root, &format!("parts/{}/parameters.json", part_of(rel))) {
        Read::Missing => Env::empty(),
        Read::Json(v, _) => ParametersFile::from_json(&v).ok()?.env(),
        Read::Bad(_) => return None,
    };
    let Read::Json(v, _) = read(root, rel) else {
        return None;
    };
    let f = Feature::from_json(&v, unit, &env).ok()?;
    if f.sketch().is_some_and(|s| !s.references().is_empty()) {
        let (_, r) = builds.rebuilt(part_of(rel))?;
        let s = r
            .output(&Id::parse(stem_of(rel), "feature").ok()?)?
            .sketch()?;
        return s
            .stale
            .then(|| canonical_bytes(&Feature::Sketch(s.sketch.clone()).to_json()));
    }
    Some(canonical_bytes(&solved_sketch(stem_of(rel), &f)?.to_json()))
}

/// A way to fix an issue: the exact file changes, and any parameters it
/// takes. Its ID names the issue, the action and the first file, plus a
/// short hash of the files involved, so a repair planned against files
/// that have since changed is refused.
#[derive(Clone, Debug, PartialEq)]
pub struct Repair {
    pub id: String,
    pub summary: String,
    pub changes: Vec<Change>,
    pub params: Vec<Param>,
}

/// One file written (with these bytes) or, with `None`, deleted; or, with
/// the path [`GIT_INSTALL`], the clone set up as `ryucad git install`
/// sets it up (its config and hooks are the clone's, not project files, so
/// this isn't logged or undone).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    pub content: Option<Vec<u8>>,
}

/// The change that runs `ryucad git install` in the project's clone.
pub const GIT_INSTALL: &str = "ryucad git install";

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: &'static str,
    pub required: bool,
    pub about: &'static str,
}

impl Repair {
    pub(crate) fn to_json(&self) -> Value {
        let changes: Vec<Value> = self
            .changes
            .iter()
            .map(|c| match &c.content {
                _ if c.path == GIT_INSTALL => json!({"action": "git_install"}),
                Some(b) => json!({"path": c.path, "action": "write",
                                  "content": String::from_utf8_lossy(b)}),
                None => json!({"path": c.path, "action": "delete"}),
            })
            .collect();
        let params: Vec<Value> = self
            .params
            .iter()
            .map(|p| json!({"name": p.name, "required": p.required, "about": p.about}))
            .collect();
        json!({"id": self.id, "summary": self.summary, "changes": changes, "params": params})
    }

    /// The ID without its hash: the same repair, whatever the files hold.
    fn key(&self) -> &str {
        key(&self.id)
    }
}

fn key(id: &str) -> &str {
    id.split_once('@').map_or(id, |(k, _)| k)
}

/// What repairs may be given.
#[derive(Clone, Debug, Default)]
pub struct Params {
    /// For `missing_manifest`: the project's default unit (mm otherwise).
    pub unit: Option<Unit>,
    /// For `missing_part_index`: every feature, in order.
    pub order: Option<Vec<String>>,
}

impl Params {
    pub fn from_json(v: &serde_json::Map<String, Value>) -> Result<Params> {
        let mut p = Params::default();
        for (k, v) in v {
            match k.as_str() {
                "unit" => {
                    let u = v.as_str().and_then(Unit::parse).ok_or_else(|| {
                        Error::new(Code::InvalidParam, "unit is one of m, cm, mm, um, in, ft")
                    })?;
                    p.unit = Some(u);
                }
                "order" => {
                    let list: Vec<String> = match v {
                        Value::String(s) => s.split(',').map(|x| x.trim().to_owned()).collect(),
                        Value::Array(a) => a
                            .iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect(),
                        _ => Vec::new(),
                    };
                    p.order = Some(list);
                }
                _ => {
                    return Err(Error::new(
                        Code::InvalidParam,
                        format!("unknown repair parameter {k:?}; use unit or order"),
                    )
                    .with("param", k));
                }
            }
        }
        Ok(p)
    }
}

/// A short hash of the files an issue involves, as they are now.
pub(crate) fn basis(root: &Path, files: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    for f in files {
        h.update(&(f.len() as u64).to_le_bytes());
        h.update(f.as_bytes());
        match fs::read(root.join(f)) {
            Ok(b) => {
                h.update(&(b.len() as u64).to_le_bytes());
                h.update(&b);
            }
            Err(_) => {
                h.update(&u64::MAX.to_le_bytes());
            }
        }
    }
    h.finalize().to_hex()[..8].to_owned()
}

pub(crate) fn canonical_bytes(v: &Value) -> Vec<u8> {
    canonical::to_string(v).into_bytes()
}

/// `parts/<part>/part.json`, if it can be read.
fn listed(root: &Path, part: &str) -> Option<PartFile> {
    match read(root, &format!("parts/{part}/part.json")) {
        Read::Json(v, _) => PartFile::from_json(&v).ok(),
        _ => None,
    }
}

/// The valid feature IDs with files in a part, sorted.
fn feature_files(root: &Path, part: &str) -> Vec<String> {
    names(&root.join(format!("parts/{part}/features")))
        .into_iter()
        .filter_map(|(n, d)| (!d).then_some(n.strip_suffix(".json")?.to_owned()))
        .filter(|s| Id::parse(s, "feature").is_ok())
        .collect()
}

/// `list` listing `features` instead: its kind and item numbers kept.
fn index_with(list: &PartFile, features: &[String]) -> Result<Vec<u8>> {
    let features = features
        .iter()
        .map(|f| Id::parse(f, "feature"))
        .collect::<Result<Vec<_>>>()?;
    Ok(canonical_bytes(
        &PartFile {
            features,
            ..list.clone()
        }
        .to_json(),
    ))
}

/// `parts/<part>/...` → `<part>`.
fn part_of(rel: &str) -> &str {
    rel.split('/').nth(1).unwrap_or("")
}

/// `.../<stem>.json` → `<stem>`.
fn stem_of(rel: &str) -> &str {
    rel.rsplit('/')
        .next()
        .and_then(|n| n.strip_suffix(".json"))
        .unwrap_or("")
}

/// The repairs for an issue, planned against the files as they are.
fn plan(
    builds: &mut Builds,
    i: &Issue,
    repo: Option<&Repo>,
    params: &Params,
) -> Result<Vec<Repair>> {
    let root = builds.root;
    // What `ryucad git install` sets up in the clone (the drivers, renames
    // off, the hooks), except a hooks folder the user chose, which ryucad
    // never writes to.
    let installs = match i.code {
        "git_drivers_missing" => Some("drivers"),
        "git_hook_missing" if !i.message.starts_with("core.hooksPath") => Some(
            crate::drivers::HOOKS
                .into_iter()
                .find(|h| i.message.contains(&format!(" no {h} hook ")))
                .unwrap_or("hooks"),
        ),
        _ => None,
    };
    if let Some(what) = installs {
        return Ok(vec![Repair {
            id: format!("{}:install:{what}", i.code),
            summary: "set this clone up for ryucad as `ryucad git install` does: the merge and diff drivers, merge.renames=false and the merge hooks (Git's config and hooks, not project files: not undoable)".to_owned(),
            changes: vec![Change {
                path: GIT_INSTALL.to_owned(),
                content: None,
            }],
            params: Vec::new(),
        }]);
    }
    let Some(first) = i.files.first() else {
        return Ok(Vec::new());
    };
    let b = basis(root, &i.files);
    let id = |action: &str| format!("{}:{action}:{first}@{b}", i.code);
    let write = |path: &str, content: Vec<u8>| Change {
        path: path.to_owned(),
        content: Some(content),
    };
    let delete = |path: &str| Change {
        path: path.to_owned(),
        content: None,
    };
    let head = |rel: &str| -> Result<Option<Vec<u8>>> {
        match repo {
            Some(r) => r.head_file(rel),
            None => Ok(None),
        }
    };
    let mut out = Vec::new();
    match i.code {
        "gitattributes_missing" => {
            let path = root.join(first);
            let text = fs::read_to_string(&path).unwrap_or_default();
            out.push(Repair {
                id: id("write"),
                summary: format!("add ryucad's lines to {first}, keeping the rest"),
                changes: vec![write(
                    first,
                    crate::drivers::with_lines_for(&text, crate::drivers::has_imports(root))
                        .into_bytes(),
                )],
                params: Vec::new(),
            });
        }
        "missing_manifest" => {
            let unit = params.unit.unwrap_or(Unit::Mm);
            out.push(Repair {
                id: id("recreate"),
                summary: format!(
                    "write a new {MANIFEST} (files keep their units, so no geometry changes)"
                ),
                changes: vec![write(
                    MANIFEST,
                    canonical_bytes(&Manifest::new(unit).to_json()),
                )],
                params: vec![Param {
                    name: "unit",
                    required: false,
                    about: "the default unit for bare numbers (mm)",
                }],
            });
        }
        "project_id_missing" => {
            if let Read::Json(v, _) = read(root, MANIFEST)
                && let Ok(m) = Manifest::from_json(&v)
            {
                let (project, whence) = match recorded_project(root) {
                    Some(p) => (p, "the one its release records name"),
                    None => (derived_project(root, &i.files), "a new one"),
                };
                let m = Manifest {
                    project: Some(project),
                    ..m
                };
                out.push(Repair {
                    id: id("write"),
                    summary: format!("give the project its ID in {MANIFEST}: {whence}"),
                    changes: vec![write(MANIFEST, canonical_bytes(&m.to_json()))],
                    params: Vec::new(),
                });
            }
        }
        "missing_part_index" => {
            let part = part_of(first);
            let files = feature_files(root, part);
            let committed = head(first)?
                .and_then(|b| canonical::parse(std::str::from_utf8(&b).ok()?).ok())
                .and_then(|v| PartFile::from_json(&v).ok());
            // An assembly comes back as one: as the last commit says, else
            // when every feature file is an instance, a mate or an
            // instance pattern.
            let instances = !files.is_empty()
                && files.iter().all(|f| {
                    matches!(read(root, &format!("parts/{part}/features/{f}.json")),
                        Read::Json(v, _) if matches!(v["type"].as_str(),
                            Some("instance" | "mate" | "instance_pattern")))
                });
            let kind = match &committed {
                Some(p) => p.kind,
                None if instances => PartKind::Assembly,
                None => PartKind::Part,
            };
            // The last commit's item numbers come back with its list.
            let base = PartFile {
                kind,
                items: committed
                    .as_ref()
                    .filter(|p| p.kind == kind)
                    .map(|p| p.items.clone())
                    .unwrap_or_default(),
                ..PartFile::default()
            };
            let committed: Vec<String> = committed
                .map(|p| p.features.iter().map(|f| f.to_string()).collect())
                .unwrap_or_default();
            let mut order: Vec<String> = committed
                .into_iter()
                .filter(|f| files.contains(f))
                .collect();
            let rest: Vec<String> = files
                .iter()
                .filter(|f| !order.contains(f))
                .cloned()
                .collect();
            let known = rest.is_empty() || files.len() == 1;
            order.extend(rest);
            if let Some(given) = &params.order {
                order = given.clone();
            }
            out.push(Repair {
                id: id("recreate"),
                summary: if known {
                    format!("write {first} listing the {} feature file(s)", files.len())
                } else {
                    format!("write {first}; the feature order can't be recovered, so give it as order=a,b,…")
                },
                changes: vec![write(first, index_with(&base, &order)?)],
                params: vec![Param {
                    name: "order",
                    required: !known,
                    about: "every feature ID, comma separated, in order",
                }],
            });
        }
        "listed_feature_missing" => {
            let (part, fid) = (part_of(first), stem_of(first));
            if let Some(list) = listed(root, part) {
                let kept: Vec<String> = list
                    .features
                    .iter()
                    .filter(|f| f.as_str() != fid)
                    .map(|f| f.to_string())
                    .collect();
                out.push(Repair {
                    id: id("remove"),
                    summary: format!("take {fid} out of parts/{part}/part.json"),
                    changes: vec![write(&i.files[1], index_with(&list, &kept)?)],
                    params: Vec::new(),
                });
            }
            if let Some(bytes) = head(first)? {
                out.push(Repair {
                    id: id("restore"),
                    summary: format!("restore {first} as the last commit has it"),
                    changes: vec![write(first, bytes)],
                    params: Vec::new(),
                });
            }
        }
        "import_file_changed" => {
            let path = i.files.last().expect("the imported file");
            if let Some(writes) = crate::import::update_in_place(root, path)? {
                let changes = writes
                    .into_iter()
                    .map(|(path, content)| Change { path, content })
                    .collect();
                out.push(Repair {
                    id: id("import_update"),
                    summary: format!(
                        "take {path} as it is now, as `import --update` would: its import features read its BLAKE3, and the parts and instances made of it follow its products and usages"
                    ),
                    changes,
                    params: Vec::new(),
                });
            }
        }
        "import_unused" => out.push(Repair {
            id: id("delete"),
            summary: format!("delete {first}, which nothing reads (Git's history keeps it)"),
            changes: vec![delete(first)],
            params: Vec::new(),
        }),
        "orphan_feature" => {
            let (part, fid) = (part_of(first), stem_of(first));
            if let Some(list) = listed(root, part) {
                let mut all: Vec<String> = list.features.iter().map(|f| f.to_string()).collect();
                all.push(fid.to_owned());
                out.push(Repair {
                    id: id("adopt"),
                    summary: format!("list {fid} last in parts/{part}/part.json"),
                    changes: vec![write(&i.files[1], index_with(&list, &all)?)],
                    params: Vec::new(),
                });
            }
            out.push(Repair {
                id: id("delete"),
                summary: format!("delete {first}"),
                changes: vec![delete(first)],
                params: Vec::new(),
            });
        }
        "invalid_json" | "invalid_document" => {
            let current = fs::read(root.join(first)).ok();
            let good = head(first)?.filter(|b| {
                Some(b) != current.as_ref()
                    && std::str::from_utf8(b).is_ok_and(|t| canonical::parse(t).is_ok())
            });
            if let Some(bytes) = good {
                out.push(Repair {
                    id: id("restore"),
                    summary: format!("restore {first} as the last commit has it"),
                    changes: vec![write(first, bytes)],
                    params: Vec::new(),
                });
            }
        }
        "sync_conflict_copy" => {
            let (copy, original) = (first, &i.files[1]);
            out.push(Repair {
                id: id("keep-original"),
                summary: format!("delete the copy {copy}"),
                changes: vec![delete(copy)],
                params: Vec::new(),
            });
            if let Ok(bytes) = fs::read(root.join(copy)) {
                out.push(Repair {
                    id: id("use-copy"),
                    summary: format!("replace {original} with {copy}, then delete the copy"),
                    changes: vec![write(original, bytes), delete(copy)],
                    params: Vec::new(),
                });
            }
        }
        "assembly_stale" => {
            let writes = builds
                .session()
                .and_then(|s| stale_assembly(s, part_of(first)));
            if let Some(writes) = writes {
                out.push(Repair {
                    id: id("solve"),
                    summary: format!("write the poses parts/{}'s mates solve to", part_of(first)),
                    changes: writes.into_iter().map(|(rel, b)| write(&rel, b)).collect(),
                    params: Vec::new(),
                });
            }
        }
        "sketch_stale" => {
            if let Some(bytes) = solved_file(builds, first) {
                out.push(Repair {
                    id: id("solve"),
                    summary: format!("write {first} with its positions solved"),
                    changes: vec![write(first, bytes)],
                    params: Vec::new(),
                });
            }
        }
        "bom_item_missing" | "bom_item_duplicate" | "bom_item_unused" => {
            use ryupdm::items;
            let part = part_of(first);
            let placed = Id::parse(part, "part").ok().and_then(|pid| {
                let project = Project::load(root).ok()?;
                Some(items::placed(project.parts.get(&pid)?))
            });
            if let (Some(list), Some(placed)) = (listed(root, part), placed) {
                let (action, numbers, summary) = match i.code {
                    "bom_item_missing" => (
                        "assign_items",
                        items::with_missing(&list.items, &placed),
                        format!(
                            "give each part parts/{part} places without an item number the next one, in list order"
                        ),
                    ),
                    "bom_item_duplicate" => (
                        "renumber_items",
                        items::deduplicated(&list.items, &placed),
                        format!(
                            "give each later part sharing an item number in parts/{part} the next one, in list order"
                        ),
                    ),
                    _ => (
                        "remove",
                        items::without_unused(&list.items, &placed),
                        format!(
                            "take the item numbers of parts parts/{part} no longer places out of {first}"
                        ),
                    ),
                };
                out.push(Repair {
                    id: id(action),
                    summary,
                    changes: vec![write(
                        first,
                        canonical_bytes(
                            &PartFile {
                                items: numbers,
                                ..list
                            }
                            .to_json(),
                        ),
                    )],
                    params: Vec::new(),
                });
            }
        }
        "non_canonical" => {
            if let Read::Json(v, _) = read(root, first) {
                out.push(Repair {
                    id: id("rewrite"),
                    summary: format!("rewrite {first} in canonical form"),
                    changes: vec![write(first, canonical_bytes(&v))],
                    params: Vec::new(),
                });
            }
        }
        _ => {}
    }
    Ok(out)
}

/// Every issue with its repairs.
fn planned(builds: &mut Builds, params: &Params) -> Result<Vec<(Issue, Vec<Repair>)>> {
    let repo = Repo::open(builds.root)?;
    diagnose_in(builds)
        .into_iter()
        .map(|i| {
            let r = plan(builds, &i, repo.as_ref(), params)?;
            Ok((i, r))
        })
        .collect()
}

fn summary(issues: &[(Issue, Vec<Repair>)]) -> Value {
    let errors = issues.iter().filter(|(i, _)| i.severity == "error").count();
    let list: Vec<Value> = issues
        .iter()
        .map(|(i, r)| {
            let mut v = i.to_json();
            v["repairs"] = json!(r.iter().map(Repair::to_json).collect::<Vec<_>>());
            v
        })
        .collect();
    json!({
        "healthy": issues.is_empty(),
        "errors": errors,
        "warnings": issues.len() - errors,
        "issues": list,
    })
}

/// `doctor`: every issue and how to repair it. Writes nothing.
pub fn report(root: &Path) -> Result<Value> {
    report_in(&mut Builds::cold(root))
}

/// `doctor`, rebuilding through `builds` (a daemon's: its session).
pub(crate) fn report_in(builds: &mut Builds) -> Result<Value> {
    Ok(summary(&planned(builds, &Params::default())?))
}

/// `doctor.repair`: applies the named repairs, in order. Every one must be
/// in a fresh diagnosis with the same hash, or nothing is done
/// (`repair_conflict`); a repair that an earlier one made unnecessary is
/// skipped.
pub fn repair(root: &Path, ids: &[String], params: &Params) -> Result<Value> {
    let now = planned(&mut Builds::cold(root), params)?;
    let all: Vec<&Repair> = now.iter().flat_map(|(_, r)| r).collect();
    for id in ids {
        let Some(r) = all.iter().find(|r| r.id == *id) else {
            return Err(Error::new(
                Code::RepairConflict,
                format!("no repair {id} now: the files changed, or it was never offered; run doctor again"),
            )
            .with("repair", id));
        };
        if let Some(p) = r
            .params
            .iter()
            .find(|p| p.required && p.name == "order" && params.order.is_none())
        {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{id} needs the parameter {}: {}", p.name, p.about),
            )
            .with("repair", id));
        }
        if r.id.starts_with("missing_part_index:")
            && let Some(order) = &params.order
        {
            let mut given = order.clone();
            given.sort();
            if given != feature_files(root, part_of(&r.changes[0].path)) {
                return Err(Error::new(
                    Code::InvalidParam,
                    "order must list every feature file of the part exactly once",
                )
                .with("repair", id));
            }
        }
    }
    let (mut applied, mut skipped, mut changes) = (Vec::new(), Vec::new(), Vec::new());
    for id in ids {
        // Planned again: an earlier repair may have changed shared files.
        // Earlier repairs changed the files: rebuild from them.
        let fresh = planned(&mut Builds::cold(root), params)?;
        let found = fresh
            .iter()
            .flat_map(|(_, r)| r)
            .find(|r| r.key() == key(id))
            .cloned();
        let Some(r) = found else {
            skipped.push(id.clone());
            continue;
        };
        for c in &r.changes {
            if c.path == GIT_INSTALL {
                crate::drivers::install(root, true)?;
                changes.push(json!({"change": "git_install"}));
                continue;
            }
            let path = root.join(&c.path);
            match &c.content {
                Some(bytes) => {
                    write_atomic(&path, bytes)?;
                    changes.push(json!({"path": c.path, "change": "written"}));
                }
                None => {
                    crate::doc::project::remove_file(root, &path)?;
                    changes.push(json!({"path": c.path, "change": "deleted"}));
                }
            }
        }
        applied.push(id.clone());
    }
    Ok(json!({
        "applied": applied,
        "skipped": skipped,
        "changes": changes,
        "report": report(root)?,
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
    fn parameters_are_checked_and_read() {
        let dir = healthy();
        let r = dir.path();
        let params = "parts/p/parameters.json";
        let a = "parts/p/features/a.json";
        let sphere = fs::read_to_string(r.join(a)).unwrap();
        let reads = sphere
            .replace("\"1 mm\"", "\"r * 2\"")
            .replace("\"format\": 1", "\"format\": 2");
        w(r, a, &reads);
        // Unknown until the parameter exists.
        assert_eq!(codes(r), [("invalid_document", vec![a.to_owned()])]);
        let good = canonical::to_string(&json!({"format": 1, "kind": "ryucad.parameters",
                                                "parameters": {"r": "1 mm"}}));
        w(r, params, &good);
        assert_eq!(codes(r), []);
        w(r, params, &good.replace('\n', "\r\n"));
        assert_eq!(codes(r), [("non_canonical", vec![params.to_owned()])]);
        // A broken parameters file is reported once, not for each reader.
        w(r, params, "{");
        assert_eq!(codes(r), [("invalid_json", vec![params.to_owned()])]);
        w(r, params, &good.replace("\"r\"", "\"R\""));
        assert_eq!(codes(r), [("invalid_document", vec![params.to_owned()])]);
        w(r, "parts/p/parameters (1).json", &good);
        assert!(codes(r).contains(&(
            "sync_conflict_copy",
            vec!["parts/p/parameters (1).json".to_owned(), params.to_owned()]
        )));
    }

    #[test]
    fn imported_files_are_checked_against_their_readers() {
        let dir = healthy();
        let r = dir.path();
        let bytes = crate::import::tests::occt("box_ap214.stp");
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let i = "parts/p/features/i.json";
        let feature = |h: &str| {
            canonical::to_string(
                &json!({"body": 0, "file": {"blake3": h, "path": "imports/box.step"},
                                         "format": 5, "kind": "ryucad.feature", "type": "import"}),
            )
        };
        w(r, i, &feature(&hash));
        w(
            r,
            "parts/p/part.json",
            &canonical::to_string(&json!({"features": ["a", "b", "i"], "format": 1,
                                          "kind": "ryucad.part"})),
        );
        let path = "imports/box.step".to_owned();
        assert_eq!(
            codes(r),
            [("import_file_missing", vec![i.to_owned(), path.clone()])]
        );
        assert!(offered(r).is_empty());
        fs::create_dir_all(r.join("imports")).unwrap();
        fs::write(r.join(&path), &bytes).unwrap();
        assert_eq!(codes(r), []);
        // Replaced by hand.
        fs::write(r.join(&path), b"ISO-10303-21;").unwrap();
        assert_eq!(
            codes(r),
            [("import_file_changed", vec![i.to_owned(), path.clone()])]
        );
        fs::write(r.join(&path), &bytes).unwrap();
        // A file nothing reads, and names that aren't imports'.
        fs::write(r.join("imports/old.step"), b"x").unwrap();
        fs::write(r.join("imports/Notes.txt"), b"x").unwrap();
        assert_eq!(
            codes(r),
            [
                ("unknown_file", vec!["imports/Notes.txt".to_owned()]),
                ("import_unused", vec!["imports/old.step".to_owned()]),
            ]
        );
        let repairs = offered(r);
        let delete = repairs
            .keys()
            .find(|k| k.starts_with("import_unused:delete:"))
            .unwrap()
            .clone();
        fix(r, &[&repairs[&delete]], Params::default());
        assert!(!r.join("imports/old.step").exists());
        fs::remove_file(r.join("imports/Notes.txt")).unwrap();
        assert_eq!(codes(r), []);
    }

    #[test]
    fn a_stale_sketch_is_solved_and_written() {
        let dir = healthy();
        let r = dir.path();
        let params = "parts/p/parameters.json";
        let s = "parts/p/features/s.json";
        let width = |w: &str| {
            canonical::to_string(&json!({"format": 1, "kind": "ryucad.parameters",
                                         "parameters": {"width": w}}))
        };
        w(r, params, &width("20 mm"));
        w(
            r,
            s,
            &canonical::to_string(&crate::doc::fixtures::rectangle()),
        );
        w(
            r,
            "parts/p/part.json",
            &canonical::to_string(&json!({"features": ["a", "b", "s"], "format": 1,
                                          "kind": "ryucad.part"})),
        );
        assert_eq!(codes(r), []);
        // Edited behind ryucad's back: reported, never written.
        w(r, params, &width("15 mm"));
        assert_eq!(
            codes(r),
            [("sketch_stale", vec![s.to_owned(), params.to_owned()])]
        );
        let before = fs::read_to_string(r.join(s)).unwrap();
        let repairs = offered(r);
        assert_eq!(fs::read_to_string(r.join(s)).unwrap(), before);
        assert_eq!(repairs.keys().collect::<Vec<_>>(), ["sketch_stale:solve:s"]);
        fix(r, &[&repairs["sketch_stale:solve:s"]], Params::default());
        assert_eq!(codes(r), []);
        // A sketch that can't be solved isn't stale, it's failing: rebuild says so.
        w(r, params, &width("50 mm"));
        assert_eq!(codes(r), []);
    }

    #[test]
    fn a_stale_projection_is_solved_in_its_part() {
        use crate::server::tests::{call, fresh};
        let (dir, mut s) = fresh();
        let r = dir.path();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
        )
        .unwrap();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "b"})).unwrap()["bodies"]
            [0]
        .clone();
        let corner = crate::sketch::tests::vertex_at(&body, [0.02, 0.01, 0.005]);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "type": "point", "projected": format!("p/b/{corner}")}
            ]}),
        )
        .unwrap();
        assert_eq!(codes(r), []);
        // The box widens behind ryucad's back.
        let b = "parts/p/features/b.json";
        let text = fs::read_to_string(r.join(b))
            .unwrap()
            .replace("20 mm", "30 mm");
        w(r, b, &text);
        let sk = "parts/p/features/s.json";
        assert_eq!(
            codes(r),
            [("sketch_stale", vec![sk.to_owned(), b.to_owned()])]
        );
        let repairs = offered(r);
        fix(r, &[&repairs["sketch_stale:solve:s"]], Params::default());
        assert_eq!(codes(r), []);
        let v: Value = serde_json::from_str(&fs::read_to_string(r.join(sk)).unwrap()).unwrap();
        assert_eq!(v["entities"]["p1"]["m"], json!([0.03, 0.01]));
    }

    #[test]
    fn every_kind_of_damage_is_named() {
        let dir = healthy();
        let r = dir.path();
        let f = |n: &str| format!("parts/p/features/{n}.json");
        let index = "parts/p/part.json".to_owned();
        let sphere = fs::read_to_string(r.join(f("a"))).unwrap();
        // An orphan, a listed feature gone, a sync copy, a bad name, an
        // unknown file, a non-canonical file, invalid JSON, conflict
        // markers, an invalid document, a newer format, a merge's
        // conflict record.
        w(r, &f("c"), &sphere);
        fs::remove_file(r.join(f("b"))).unwrap();
        w(r, "parts/p/features/a (1).json", &sphere);
        w(r, "parts/p/features/Hole.json", &sphere);
        w(r, "parts/p/features/notes.txt", "hi");
        // ryupdm's properties belong in a part; a sync copy of them doesn't.
        let props = canonical::to_string(&json!({"format": 1, "kind": "ryupdm.properties"}));
        w(r, "parts/p/properties.json", &props);
        w(r, "parts/p/properties (1).json", &props);
        w(r, &f("d"), &sphere.replace('\n', "\r\n"));
        w(r, &f("e"), "{");
        w(r, &f("g"), "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> other\n");
        w(r, &f("h"), "{}");
        w(
            r,
            &f("i"),
            &sphere.replace("\"format\": 1", "\"format\": 9"),
        );
        w(
            r,
            &f("j"),
            &sphere.replace(
                "{\n",
                "{\n  \"conflicts\": {\n    \"/params/radius\": {\n      \"kind\": \"value\"\n    }\n  },\n",
            ),
        );
        let got = codes(r);
        let want: Vec<(&str, Vec<String>)> = vec![
            (
                "sync_conflict_copy",
                vec![
                    "parts/p/properties (1).json".into(),
                    "parts/p/properties.json".into(),
                ],
            ),
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
            ("merge_conflict", vec![f("j")]),
            ("listed_feature_missing", vec![f("b"), index.clone()]),
            ("orphan_feature", vec![f("c"), index.clone()]),
            ("orphan_feature", vec![f("d"), index.clone()]),
            ("orphan_feature", vec![f("e"), index.clone()]),
            ("orphan_feature", vec![f("g"), index.clone()]),
            ("orphan_feature", vec![f("h"), index.clone()]),
            ("orphan_feature", vec![f("i"), index.clone()]),
            ("orphan_feature", vec![f("j"), index.clone()]),
        ];
        assert_eq!(got, want);
        let rep = report(r).unwrap();
        assert_eq!(rep["healthy"], false);
        assert!(rep["errors"].as_u64().unwrap() > 0);
    }

    #[test]
    fn names_that_differ_only_in_case_collide() {
        let dir = healthy();
        let r = dir.path();
        w(r, "parts/Q/part.json", "{}");
        if r.join("parts/q").exists() {
            // A case-insensitive file system (macOS and Windows by
            // default) can't hold both names, which is the point.
            return;
        }
        w(
            r,
            "parts/q/part.json",
            r#"{"features":[],"format":1,"kind":"ryucad.part"}"#,
        );
        let want: Vec<(&str, Vec<String>)> = vec![
            // Byte order: upper case first.
            ("case_collision", vec!["parts/Q".into(), "parts/q".into()]),
            // Written by hand above, so not canonical.
            ("non_canonical", vec!["parts/q/part.json".into()]),
        ];
        assert_eq!(codes(r), want);
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

    /// The IDs of the repairs offered now, by issue code and action.
    fn offered(root: &Path) -> BTreeMap<String, String> {
        let v = report(root).unwrap();
        let mut out = BTreeMap::new();
        for i in v["issues"].as_array().unwrap() {
            for r in i["repairs"].as_array().unwrap() {
                let id = r["id"].as_str().unwrap().to_owned();
                let mut parts = id.splitn(3, ':');
                let (code, action) = (parts.next().unwrap(), parts.next().unwrap());
                out.insert(
                    format!(
                        "{code}:{action}:{}",
                        stem_of(i["files"][0].as_str().unwrap())
                    ),
                    id,
                );
            }
        }
        out
    }

    fn fix(root: &Path, ids: &[&String], params: Params) -> Value {
        let ids: Vec<String> = ids.iter().map(|s| (*s).clone()).collect();
        repair(root, &ids, &params).unwrap()
    }

    #[test]
    fn pdm_files_are_read_and_judged() {
        let dir = healthy();
        let r = dir.path();
        let mut s = crate::session::Session::open(r).unwrap();
        call(&mut s, "part.create", json!({"part": "q"})).unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": "1"}}),
        )
        .unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "q", "properties": {"number": "2"}}),
        )
        .unwrap();
        assert!(codes(r).is_empty(), "{:?}", codes(r));
        // Two parts with one number, and a property nobody declared: by
        // hand, as a merge might leave them.
        w(
            r,
            "parts/q/properties.json",
            &canonical::to_string(
                &json!({"format": 1, "kind": "ryupdm.properties", "number": "1", "x": 1}),
            ),
        );
        let p = "parts/p/properties.json".to_owned();
        let q = "parts/q/properties.json".to_owned();
        assert_eq!(
            codes(r),
            [
                ("part_number_duplicate", vec![p.clone(), q.clone()]),
                ("property_unknown", vec![q.clone()]),
                ("part_number_duplicate", vec![q.clone(), p.clone()]),
            ]
        );
        w(r, &q, "{");
        w(
            r,
            "ryupdm.json",
            "{\"format\": 1, \"kind\": \"ryupdm.config\", \"revisions\": \"roman\"}",
        );
        assert_eq!(
            codes(r),
            [
                ("invalid_document", vec!["ryupdm.json".into()]),
                ("invalid_json", vec![q.clone()]),
            ]
        );
        assert_eq!(report(r).unwrap()["errors"], 2);
    }

    #[test]
    fn materials_are_read_and_parts_judged() {
        let dir = healthy();
        let r = dir.path();
        let mut s = crate::session::Session::open(r).unwrap();
        let material = |s: &mut crate::session::Session, m: &str| {
            call(
                s,
                "props.set",
                json!({"part": "p", "properties": {"material": m}}),
            )
            .unwrap();
        };
        // No library: nothing to judge a material by.
        material(&mut s, "brass");
        assert!(codes(r).is_empty(), "{:?}", codes(r));
        let library = json!({"format": 1, "kind": "ryucad.materials",
                             "materials": {"steel": {"density": "7850 kg/m^3"}}});
        w(r, "materials.json", &canonical::to_string(&library));
        let both = vec![
            "parts/p/properties.json".to_owned(),
            "materials.json".to_owned(),
        ];
        assert_eq!(codes(r), [("material_unknown", both)]);
        material(&mut s, "steel");
        assert!(codes(r).is_empty(), "{:?}", codes(r));
        w(
            r,
            "materials.json",
            "{\"format\": 1, \"kind\": \"ryucad.materials\"}",
        );
        assert_eq!(
            codes(r),
            [("invalid_document", vec!["materials.json".to_owned()])]
        );
        w(
            r,
            "materials.json",
            &serde_json::to_string(&library).unwrap(),
        );
        assert_eq!(
            codes(r),
            [("non_canonical", vec!["materials.json".to_owned()])]
        );
    }

    /// A release record of `part`'s `label`, as ryupdm writes one.
    fn record(part: &str, label: &str) -> Value {
        json!({"built_with": {"commit": "c", "ryucad": "0", "ryukei": "0"},
               "content": "c".repeat(64), "format": 1, "kind": "ryupdm.release",
               "label": label, "part": part, "project": "0123456789abcdef0123456789abcdef",
               "properties": {}, "released_by": {"participant": "agent"}, "results": {},
               "supersedes": null})
    }

    #[test]
    fn release_records_are_read_where_they_lie() {
        let dir = healthy();
        let r = dir.path();
        let mut s = crate::session::Session::open(r).unwrap();
        call(&mut s, "pdm.init", json!({})).unwrap();
        fs::create_dir_all(r.join("releases/p")).unwrap();
        let a = canonical::to_string(&record("p", "A"));
        w(r, "releases/p/A.json", &a);
        assert!(codes(r).is_empty(), "{:?}", codes(r));
        fs::create_dir_all(r.join("releases/Bad Name")).unwrap();
        let mut newer = record("p", "C");
        newer["format"] = json!(2);
        w(r, "releases/p/A (1).json", &a);
        w(
            r,
            "releases/p/B.json",
            &a.replace('\n', "\r\n").replace("\"A\"", "\"B\""),
        );
        w(r, "releases/p/C.json", &canonical::to_string(&newer));
        w(
            r,
            "releases/p/D.json",
            &canonical::to_string(&record("q", "D")),
        );
        w(r, "releases/p/E.obsolete.json", "{");
        w(r, "releases/p/notes.txt", "hi");
        w(r, "releases/stray.json", "{}");
        let p = |n: &str| format!("releases/p/{n}");
        assert_eq!(
            codes(r),
            [
                ("unknown_file", vec!["releases/Bad Name".into()]),
                ("unknown_file", vec!["releases/stray.json".into()]),
                ("sync_conflict_copy", vec![p("A (1).json"), p("A.json")]),
                ("unknown_file", vec![p("notes.txt")]),
                ("non_canonical", vec![p("B.json")]),
                ("format_too_new", vec![p("C.json")]),
                ("invalid_document", vec![p("D.json")]),
                ("invalid_json", vec![p("E.obsolete.json")]),
            ]
        );
    }

    #[test]
    fn a_project_that_lost_its_id_gets_it_back() {
        let dir = healthy();
        let r = dir.path();
        let mut s = crate::session::Session::open(r).unwrap();
        let id = call(&mut s, "pdm.init", json!({})).unwrap()["project"].clone();
        let with_id = fs::read_to_string(r.join(MANIFEST)).unwrap();
        let without = canonical::to_string(&Manifest::new(Unit::Mm).to_json());
        w(r, MANIFEST, &without);
        let files = vec![MANIFEST.to_owned(), PDM_CONFIG.to_owned()];
        assert_eq!(codes(r), [("project_id_missing", files.clone())]);
        // With no record naming one, a new ID: the same in the report and
        // the repair.
        let rep = report(r).unwrap();
        let shown = rep["issues"][0]["repairs"][0]["changes"][0]["content"].clone();
        let new = offered(r);
        fix(
            r,
            &[&new["project_id_missing:write:ryucad"]],
            Params::default(),
        );
        assert_eq!(
            fs::read_to_string(r.join(MANIFEST)).unwrap(),
            shown.as_str().unwrap()
        );
        assert!(codes(r).is_empty());
        // With a record, its ID.
        w(r, MANIFEST, &without);
        fs::create_dir_all(r.join("releases/p")).unwrap();
        w(r, "releases/p/A.json", &format!("{{\"project\": {id}}}"));
        let new = offered(r);
        fix(
            r,
            &[&new["project_id_missing:write:ryucad"]],
            Params::default(),
        );
        assert_eq!(fs::read_to_string(r.join(MANIFEST)).unwrap(), with_id);
    }

    #[test]
    fn orphans_are_adopted_or_deleted_and_copies_resolved() {
        let dir = healthy();
        let r = dir.path();
        let a = fs::read_to_string(r.join("parts/p/features/a.json")).unwrap();
        w(r, "parts/p/features/c.json", &a);
        w(r, "parts/p/features/d.json", &a);
        w(r, "parts/p/features/b (1).json", &a.replace("1 mm", "5 mm"));
        let ids = offered(r);
        // Planning writes nothing.
        assert_eq!(diagnose(r).len(), 3);
        // Two repairs on one part.json compose, and a copy is taken.
        let out = fix(
            r,
            &[
                &ids["orphan_feature:adopt:c"],
                &ids["orphan_feature:delete:d"],
                &ids["sync_conflict_copy:use-copy:b (1)"],
            ],
            Params::default(),
        );
        assert_eq!(out["skipped"], json!([]));
        assert_eq!(out["report"]["healthy"], true, "{out}");
        let list = listed(r, "p").unwrap().features;
        assert_eq!(
            list.iter().map(Id::as_str).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert!(!r.join("parts/p/features/d.json").exists());
        let b = fs::read_to_string(r.join("parts/p/features/b.json")).unwrap();
        assert!(b.contains("5 mm"));
    }

    #[test]
    fn a_repair_planned_on_old_files_is_refused() {
        let dir = healthy();
        let r = dir.path();
        let a = fs::read_to_string(r.join("parts/p/features/a.json")).unwrap();
        w(r, "parts/p/features/c.json", &a);
        let ids = offered(r);
        w(r, "parts/p/features/c.json", &a.replace("1 mm", "2 mm"));
        let e = repair(
            r,
            &[ids["orphan_feature:adopt:c"].clone()],
            &Params::default(),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::RepairConflict);
        // Nothing was done.
        assert_eq!(listed(r, "p").unwrap().features.len(), 2);
    }

    #[test]
    fn a_lost_assembly_list_comes_back_as_an_assembly() {
        let dir = healthy();
        let r = dir.path();
        w(
            r,
            "parts/frame/features/i1.json",
            &canonical::to_string(&json!({
                "format": 4, "kind": "ryucad.feature", "type": "instance", "part": "p",
                "rev": "working",
                "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]},
            })),
        );
        let ids = offered(r);
        let out = fix(
            r,
            &[&ids["missing_part_index:recreate:part"]],
            Params::default(),
        );
        assert_eq!(out["skipped"], json!([]), "{out}");
        let list = listed(r, "frame").unwrap();
        assert_eq!(list.kind, PartKind::Assembly);
        assert_eq!(list.features.len(), 1);
    }

    #[test]
    fn item_numbers_missing_shared_or_unused_are_repaired() {
        let dir = healthy();
        let r = dir.path();
        let mut s = crate::session::Session::open(r).unwrap();
        call(&mut s, "part.create", json!({"part": "q"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "q", "type": "sphere", "id": "a", "params": {"radius": 1}}),
        )
        .unwrap();
        call(
            &mut s,
            "part.create",
            json!({"part": "frame", "kind": "assembly"}),
        )
        .unwrap();
        for (inst, of) in [("i1", "p"), ("i2", "q")] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "frame", "type": "instance", "id": inst, "of": of}),
            )
            .unwrap();
        }
        assert_eq!(codes(r), []);
        let numbered = |items: Value| {
            let mut v: Value =
                serde_json::from_str(&fs::read_to_string(r.join("parts/frame/part.json")).unwrap())
                    .unwrap();
            v["items"] = items;
            w(r, "parts/frame/part.json", &canonical::to_string(&v));
        };
        let items = || listed(r, "frame").unwrap().items;
        let file = || vec!["parts/frame/part.json".to_owned()];
        // A hand edit drops q's number and numbers a part nothing places.
        numbered(json!({"p": 1, "gone": 2}));
        assert_eq!(
            codes(r),
            [("bom_item_missing", file()), ("bom_item_unused", file())]
        );
        let ids = offered(r);
        fix(
            r,
            &[
                &ids["bom_item_missing:assign_items:part"],
                &ids["bom_item_unused:remove:part"],
            ],
            Params::default(),
        );
        let id = |p: &str| Id::parse(p, "part").unwrap();
        assert_eq!(items(), [(id("p"), 1), (id("q"), 3)].into_iter().collect());
        // A merge leaves both parts at 1: the later one in the list moves.
        numbered(json!({"p": 1, "q": 1}));
        assert_eq!(codes(r), [("bom_item_duplicate", file())]);
        let ids = offered(r);
        let out = fix(
            r,
            &[&ids["bom_item_duplicate:renumber_items:part"]],
            Params::default(),
        );
        assert_eq!(out["report"]["healthy"], true, "{out}");
        assert_eq!(items(), [(id("p"), 1), (id("q"), 2)].into_iter().collect());
    }

    #[test]
    fn list_repairs_keep_an_assemblys_item_numbers() {
        let dir = healthy();
        let r = dir.path();
        let instance = canonical::to_string(&json!({
            "format": 4, "kind": "ryucad.feature", "type": "instance", "part": "p",
            "rev": "working",
            "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]},
        }));
        w(r, "parts/frame/features/i1.json", &instance);
        w(r, "parts/frame/features/i2.json", &instance);
        w(
            r,
            "parts/frame/part.json",
            &canonical::to_string(&json!({"features": ["i1"], "format": 3,
                "items": {"p": 4}, "kind": "ryucad.part", "part_kind": "assembly"})),
        );
        let ids = offered(r);
        fix(r, &[&ids["orphan_feature:adopt:i2"]], Params::default());
        let list = listed(r, "frame").unwrap();
        assert_eq!(list.features.len(), 2);
        assert_eq!(list.items[&Id::parse("p", "part").unwrap()], 4);
    }

    #[test]
    fn a_lost_manifest_and_index_come_back() {
        let dir = healthy();
        let r = dir.path();
        fs::remove_file(r.join(MANIFEST)).unwrap();
        fs::remove_file(r.join("parts/p/part.json")).unwrap();
        let ids = offered(r);
        // Two features and no history: the order must be given.
        let index = ids["missing_part_index:recreate:part"].clone();
        let e = repair(r, std::slice::from_ref(&index), &Params::default()).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let bad = Params {
            order: Some(vec!["b".into()]),
            ..Params::default()
        };
        assert_eq!(
            repair(r, std::slice::from_ref(&index), &bad)
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
        let params = Params {
            unit: Some(Unit::In),
            order: Some(vec!["b".into(), "a".into()]),
        };
        let out = fix(
            r,
            &[&ids["missing_manifest:recreate:ryucad"], &index],
            params,
        );
        assert_eq!(out["report"]["healthy"], true, "{out}");
        let list = listed(r, "p").unwrap().features;
        assert_eq!(list.iter().map(Id::as_str).collect::<Vec<_>>(), ["b", "a"]);
        let m = fs::read_to_string(r.join(MANIFEST)).unwrap();
        assert!(m.contains("\"in\""));
        // The project opens again.
        crate::session::Session::open(r).unwrap();
    }

    /// Makes `r` a repository that registers ryucad with Git, whatever the
    /// user's own config says.
    /// Makes `r` a repository set up for ryucad, by hand: the test binary
    /// isn't ryucad, so `git install` would register the wrong program.
    pub(crate) fn in_repo(r: &Path) {
        use crate::drivers::{HOOKS, MERGE_DRIVER, RENAMES, TEXTCONV};
        let git = |args: &[&str]| crate::git::tests::git(r, args);
        git(&["init", "-q"]);
        git(&["config", RENAMES, "false"]);
        git(&["config", TEXTCONV, "ryucad git textconv"]);
        git(&[
            "config",
            MERGE_DRIVER,
            "ryucad git merge-driver %O %A %B %P",
        ]);
        let hooks = r.join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        for h in HOOKS {
            // Someone's own hook that runs the check, harmless when Git
            // runs it (Git for Windows runs hooks that aren't executable).
            fs::write(hooks.join(h), "#!/bin/sh\n: ryucad merge status --hook\n").unwrap();
        }
        crate::drivers::write_attributes(r).unwrap();
    }

    #[test]
    fn a_repository_needs_the_attributes() {
        let dir = healthy();
        let r = dir.path();
        // Outside a repository, Git's files don't matter.
        assert_eq!(report(r).unwrap()["healthy"], true);
        in_repo(r);
        assert_eq!(report(r).unwrap()["healthy"], true);
        // An earlier ryucad's lines: the merge driver is missing.
        w(
            r,
            crate::drivers::ATTRIBUTES,
            "*.png binary\nryucad.json diff=ryucad\n",
        );
        let v = report(r).unwrap();
        assert_eq!(v["issues"][0]["code"], "gitattributes_missing");
        assert_eq!(v["issues"][0]["severity"], "warning");
        assert!(
            v["issues"][0]["message"]
                .as_str()
                .unwrap()
                .contains("parts/**/*.json merge=ryucad diff=ryucad text eol=lf")
        );
        let ids = offered(r);
        let out = fix(
            r,
            &[&ids["gitattributes_missing:write:"]],
            Params::default(),
        );
        assert_eq!(out["report"]["healthy"], true, "{out}");
        let text = fs::read_to_string(r.join(crate::drivers::ATTRIBUTES)).unwrap();
        assert!(text.starts_with("*.png binary\n# ryucad"), "{text}");
        assert!(!text.contains("\nryucad.json diff=ryucad\n"), "{text}");
    }

    /// Issue #163: the message names the lines missing, each whole and
    /// quoted, or says there is no file.
    #[test]
    fn the_attributes_message_names_what_is_missing() {
        let dir = healthy();
        let r = dir.path();
        in_repo(r);
        let message = |r: &Path| {
            let v = report(r).unwrap();
            assert_eq!(v["issues"][0]["code"], "gitattributes_missing", "{v}");
            v["issues"][0]["message"].as_str().unwrap().to_owned()
        };
        let why = ": Git won't read or merge the project's files through ryucad";
        fs::remove_file(r.join(crate::drivers::ATTRIBUTES)).unwrap();
        assert_eq!(message(r), format!("there is no .gitattributes{why}"));
        // Every line but one.
        let all = crate::drivers::with_lines("");
        w(
            r,
            crate::drivers::ATTRIBUTES,
            &all.replace(".gitattributes text eol=lf\n", ""),
        );
        assert_eq!(
            message(r),
            format!(".gitattributes lacks the line \".gitattributes text eol=lf\"{why}")
        );
        // An earlier ryucad's lines: every line is missing.
        w(
            r,
            crate::drivers::ATTRIBUTES,
            "ryucad.json diff=ryucad\nparts/**/*.json diff=ryucad\n",
        );
        let quoted: Vec<String> = crate::drivers::LINES
            .iter()
            .map(|l| format!("\"{l}\""))
            .collect();
        let (last, rest) = quoted.split_last().unwrap();
        assert_eq!(
            message(r),
            format!(
                ".gitattributes lacks the lines {} and {last}{why}",
                rest.join(", ")
            )
        );
    }

    #[test]
    fn a_clone_needs_the_drivers_and_the_hook() {
        let dir = healthy();
        let r = dir.path();
        in_repo(r);
        let git = |args: &[&str]| crate::git::tests::git(r, args);
        git(&["config", "--unset", crate::drivers::MERGE_DRIVER]);
        let hook = r.join(".git/hooks").join(crate::drivers::HOOKS[0]);
        fs::write(&hook, "#!/bin/sh\necho someone else's\n").unwrap();
        let v = report(r).unwrap();
        let codes: Vec<&str> = v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["code"].as_str().unwrap())
            .collect();
        assert_eq!(codes, ["git_drivers_missing", "git_hook_foreign"]);
        assert!(
            v["issues"][0]["message"]
                .as_str()
                .unwrap()
                .contains("merge.ryucad.driver")
        );
        assert!(
            v["issues"][1]["message"]
                .as_str()
                .unwrap()
                .ends_with("' merge status --hook || exit 1")
        );
        fs::remove_file(&hook).unwrap();
        let v = report(r).unwrap();
        assert_eq!(v["issues"][1]["code"], "git_hook_missing");
        assert_eq!(v["warnings"], 2);
        // Renames left on are the drivers' issue too.
        git(&["config", crate::drivers::RENAMES, "true"]);
        let v = report(r).unwrap();
        let message = v["issues"][0]["message"].as_str().unwrap();
        assert!(message.contains("merge.renames"), "{message}");
        // A fresh clone's repair sets it all up, as `git install` there.
        let hook_fix = &v["issues"][1]["repairs"][0];
        assert_eq!(hook_fix["id"], "git_hook_missing:install:pre-merge-commit");
        assert_eq!(hook_fix["changes"], json!([{"action": "git_install"}]));
        let ids = [
            v["issues"][0]["repairs"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned(),
            hook_fix["id"].as_str().unwrap().to_owned(),
        ];
        let out = repair(r, &ids, &Params::default()).unwrap();
        // The first did both; the second had nothing left to do.
        assert_eq!(out["applied"], json!([ids[0]]));
        assert_eq!(out["skipped"], json!([ids[1]]));
        assert_eq!(out["report"]["healthy"], true, "{out}");
        assert!(crate::drivers::setup(r).unwrap().missing.is_empty());
    }

    #[test]
    fn the_last_commit_restores_what_was_lost() {
        let dir = healthy();
        let r = dir.path();
        in_repo(r);
        crate::git::tests::git(r, &["add", "."]);
        crate::git::tests::git(r, &["commit", "-q", "-m", "start"]);
        // A deleted file, a broken one, and a lost index with history.
        fs::remove_file(r.join("parts/p/features/a.json")).unwrap();
        w(r, "parts/p/features/b.json", "{");
        let ids = offered(r);
        let out = fix(
            r,
            &[
                &ids["listed_feature_missing:restore:a"],
                &ids["invalid_json:restore:b"],
            ],
            Params::default(),
        );
        assert_eq!(out["report"]["healthy"], true, "{out}");
        fs::remove_file(r.join("parts/p/part.json")).unwrap();
        let ids = offered(r);
        let v = report(r).unwrap();
        assert_eq!(v["issues"][0]["repairs"][0]["params"][0]["required"], false);
        let out = fix(
            r,
            &[&ids["missing_part_index:recreate:part"]],
            Params::default(),
        );
        assert_eq!(out["report"]["healthy"], true);
        let list = listed(r, "p").unwrap();
        assert_eq!(
            list.features.iter().map(Id::as_str).collect::<Vec<_>>(),
            ["a", "b"]
        );
        // Removing a listed feature that's gone is the other way.
        fs::remove_file(r.join("parts/p/features/a.json")).unwrap();
        let ids = offered(r);
        fix(
            r,
            &[&ids["listed_feature_missing:remove:a"]],
            Params::default(),
        );
        assert_eq!(listed(r, "p").unwrap().features.len(), 1);
    }

    #[test]
    fn repairs_take_known_parameters_only() {
        let p = |v: Value| Params::from_json(v.as_object().unwrap());
        assert_eq!(p(json!({"unit": "cm"})).unwrap().unit, Some(Unit::Cm));
        assert_eq!(
            p(json!({"order": "a, b"})).unwrap().order.unwrap(),
            ["a", "b"]
        );
        assert_eq!(
            p(json!({"colour": "red"})).unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(
            p(json!({"unit": "parsec"})).unwrap_err().code,
            Code::InvalidParam
        );
    }
}
