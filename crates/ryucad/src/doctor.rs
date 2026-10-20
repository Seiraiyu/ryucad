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
use crate::doc::files::{Feature, Manifest, ParametersFile, PartFile};
use crate::doc::id::Id;
use crate::doc::project::{MANIFEST, write_atomic};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::git::Repo;
use crate::rebuild::PartResult;
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
pub const CODES: [(&str, &str); 15] = [
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
        |n, d| (n == "part.json" && !d) || (n == "parameters.json" && !d) || (n == "features" && d),
        &["part", "parameters"],
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
                        let in_part = in_part.get_or_insert_with(|| stale_in_part(root, part));
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

/// The part's rebuild from the files as they stand, if the project loads.
fn rebuilt(root: &Path, part: &str) -> Option<(crate::doc::project::Part, PartResult)> {
    let project = crate::doc::project::Project::load(root).ok()?;
    let p = project.parts.get(&Id::parse(part, "part").ok()?)?.clone();
    let r = crate::rebuild::rebuild_part(&p);
    Some((p, r))
}

/// The stale sketches of `part` that read other features, each with the
/// files of the features it reads, directly or not. Empty when the
/// project can't be loaded.
fn stale_in_part(root: &Path, part: &str) -> BTreeMap<String, Vec<String>> {
    let Some((p, r)) = rebuilt(root, part) else {
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
fn solved_file(root: &Path, rel: &str) -> Option<Vec<u8>> {
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
        let (_, r) = rebuilt(root, part_of(rel))?;
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

/// One file written (with these bytes) or, with `None`, deleted.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    pub content: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: &'static str,
    pub required: bool,
    pub about: &'static str,
}

impl Repair {
    fn to_json(&self) -> Value {
        let changes: Vec<Value> = self
            .changes
            .iter()
            .map(|c| match &c.content {
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
fn basis(root: &Path, files: &[String]) -> String {
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

fn canonical_bytes(v: &Value) -> Vec<u8> {
    canonical::to_string(v).into_bytes()
}

/// The feature list in `parts/<part>/part.json`, if it can be read.
fn listed(root: &Path, part: &str) -> Option<Vec<Id>> {
    match read(root, &format!("parts/{part}/part.json")) {
        Read::Json(v, _) => PartFile::from_json(&v).ok().map(|p| p.features),
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

fn index_with(features: &[String]) -> Result<Vec<u8>> {
    let features = features
        .iter()
        .map(|f| Id::parse(f, "feature"))
        .collect::<Result<Vec<_>>>()?;
    Ok(canonical_bytes(&PartFile { features }.to_json()))
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
fn plan(root: &Path, i: &Issue, repo: Option<&Repo>, params: &Params) -> Result<Vec<Repair>> {
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
        "missing_manifest" => {
            let unit = params.unit.unwrap_or(Unit::Mm);
            out.push(Repair {
                id: id("recreate"),
                summary: format!(
                    "write a new {MANIFEST} (files keep their units, so no geometry changes)"
                ),
                changes: vec![write(
                    MANIFEST,
                    canonical_bytes(&Manifest { default_unit: unit }.to_json()),
                )],
                params: vec![Param {
                    name: "unit",
                    required: false,
                    about: "the default unit for bare numbers (mm)",
                }],
            });
        }
        "missing_part_index" => {
            let part = part_of(first);
            let files = feature_files(root, part);
            let committed: Vec<String> = head(first)?
                .and_then(|b| canonical::parse(std::str::from_utf8(&b).ok()?).ok())
                .and_then(|v| PartFile::from_json(&v).ok())
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
                changes: vec![write(first, index_with(&order)?)],
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
                    .iter()
                    .filter(|f| f.as_str() != fid)
                    .map(|f| f.to_string())
                    .collect();
                out.push(Repair {
                    id: id("remove"),
                    summary: format!("take {fid} out of parts/{part}/part.json"),
                    changes: vec![write(&i.files[1], index_with(&kept)?)],
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
        "orphan_feature" => {
            let (part, fid) = (part_of(first), stem_of(first));
            if let Some(list) = listed(root, part) {
                let mut all: Vec<String> = list.iter().map(|f| f.to_string()).collect();
                all.push(fid.to_owned());
                out.push(Repair {
                    id: id("adopt"),
                    summary: format!("list {fid} last in parts/{part}/part.json"),
                    changes: vec![write(&i.files[1], index_with(&all)?)],
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
        "sketch_stale" => {
            if let Some(bytes) = solved_file(root, first) {
                out.push(Repair {
                    id: id("solve"),
                    summary: format!("write {first} with its positions solved"),
                    changes: vec![write(first, bytes)],
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
fn planned(root: &Path, params: &Params) -> Result<Vec<(Issue, Vec<Repair>)>> {
    let repo = Repo::open(root)?;
    diagnose(root)
        .into_iter()
        .map(|i| {
            let r = plan(root, &i, repo.as_ref(), params)?;
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
    Ok(summary(&planned(root, &Params::default())?))
}

/// `doctor.repair`: applies the named repairs, in order. Every one must be
/// in a fresh diagnosis with the same hash, or nothing is done
/// (`repair_conflict`); a repair that an earlier one made unnecessary is
/// skipped.
pub fn repair(root: &Path, ids: &[String], params: &Params) -> Result<Value> {
    let now = planned(root, params)?;
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
        let fresh = planned(root, params)?;
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
            &canonical::to_string(&crate::doc::sketch::tests::rectangle()),
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
        // markers, an invalid document, a newer format.
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
        let got = codes(r);
        let want: Vec<(&str, Vec<String>)> = vec![
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
        let list = listed(r, "p").unwrap();
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
        assert_eq!(listed(r, "p").unwrap().len(), 2);
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
        let list = listed(r, "p").unwrap();
        assert_eq!(list.iter().map(Id::as_str).collect::<Vec<_>>(), ["b", "a"]);
        let m = fs::read_to_string(r.join(MANIFEST)).unwrap();
        assert!(m.contains("\"in\""));
        // The project opens again.
        crate::session::Session::open(r).unwrap();
    }

    #[test]
    fn the_last_commit_restores_what_was_lost() {
        let dir = healthy();
        let r = dir.path();
        crate::git::tests::git(r, &["init", "-q"]);
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
        assert_eq!(list.iter().map(Id::as_str).collect::<Vec<_>>(), ["a", "b"]);
        // Removing a listed feature that's gone is the other way.
        fs::remove_file(r.join("parts/p/features/a.json")).unwrap();
        let ids = offered(r);
        fix(
            r,
            &[&ids["listed_feature_missing:remove:a"]],
            Params::default(),
        );
        assert_eq!(listed(r, "p").unwrap().len(), 1);
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
