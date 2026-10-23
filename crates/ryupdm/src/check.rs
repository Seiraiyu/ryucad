//! The release checks: every record the checkout holds against the parts,
//! the release branch and the tags. Findings have a fixed shape, version
//! [`SCHEMA`]: `{code, severity, source, part?, label?, message,
//! details?}`, `source` always `release`. Adding a field or a code is not
//! a new schema; changing or removing one is.
//!
//! | Code | Severity | Meaning |
//! |---|---|---|
//! | `release_hash_mismatch` | error | An official record's content or properties differ from the part at its official commit |
//! | `release_stale` | error | A proposed record differs from the working part |
//! | `release_modified` | error | An official record was changed or deleted later |
//! | `release_sequence` | error | A label isn't the scheme's next after `supersedes`, `supersedes` isn't the part's previous release, or an obsolete record names no release |
//! | `release_project_mismatch` | error | A record's `project` isn't the manifest's |
//! | `release_part_deleted` | error | A part folder is gone while its latest official release isn't obsolete |
//! | `release_result_changed` | warning | This build rebuilds an official release's content to other body hashes |
//! | `release_tag_missing` | warning | An official release has no tag |
//! | `release_tag_moved` | warning | A tag points away from its release's official commit |
//! | `release_tag_orphaned` | warning | A `ryupdm/` tag names no official release |

use crate::config::Config;
use crate::history::{Releases, Tag, tag_name};
use crate::label;
use crate::props;
use crate::record::{Entry, Kind, Record, Records};
use crate::state;
use ryucad_doc::doc::files::Manifest;
use ryucad_doc::doc::id::{Id, ProjectId};
use ryucad_doc::doc::project::MANIFEST;
use ryucad_doc::doc::source::Source;
use ryucad_doc::error::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The version of the findings' shape.
pub const SCHEMA: u64 = 1;

/// Every code a release check reports, with its severity.
pub const CODES: [(&str, &str); 10] = [
    ("release_hash_mismatch", "error"),
    ("release_stale", "error"),
    ("release_modified", "error"),
    ("release_sequence", "error"),
    ("release_project_mismatch", "error"),
    ("release_part_deleted", "error"),
    ("release_result_changed", "warning"),
    ("release_tag_missing", "warning"),
    ("release_tag_moved", "warning"),
    ("release_tag_orphaned", "warning"),
];

/// One thing a check found.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: &'static str,
    pub part: Option<Id>,
    pub label: Option<String>,
    pub message: String,
    /// More about it, as an object, or `Null`.
    pub details: Value,
}

impl Finding {
    fn new(code: &'static str, part: &Id, label: Option<&str>, message: String) -> Finding {
        assert!(CODES.iter().any(|(c, _)| *c == code), "a known code");
        Finding {
            code,
            part: Some(part.clone()),
            label: label.map(str::to_owned),
            message,
            details: Value::Null,
        }
    }

    fn with(mut self, key: &str, value: impl serde::Serialize) -> Finding {
        if !self.details.is_object() {
            self.details = json!({});
        }
        self.details[key] = json!(value);
        self
    }

    pub fn severity(&self) -> &'static str {
        CODES
            .iter()
            .find(|(c, _)| *c == self.code)
            .map(|(_, s)| *s)
            .expect("a known code")
    }

    /// The finding in the schema's shape.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"code": self.code, "severity": self.severity(), "source": "release",
                           "message": self.message});
        if let Some(p) = &self.part {
            v["part"] = json!(p);
        }
        if let Some(l) = &self.label {
            v["label"] = json!(l);
        }
        if !self.details.is_null() {
            v["details"] = self.details.clone();
        }
        v
    }
}

/// Rebuilds old content for `release_result_changed`: the body hash of
/// each feature of `part` that makes a body, rebuilt by this build from
/// `src`, the project as a release's official commit has it. `content` is
/// that part's content hash, by which results may be cached.
pub trait Results {
    fn bodies(
        &mut self,
        src: &dyn Source,
        part: &Id,
        content: &str,
    ) -> Result<BTreeMap<String, String>>;
}

/// The repository a project is in, where it sits there (ending in `/`
/// unless at the root), and its release branch as read.
pub struct History<'a> {
    pub repo: &'a gix::Repository,
    pub prefix: &'a str,
    pub releases: &'a Releases,
}

/// Bytes equal once CRLF line endings are read as LF.
fn same_text(a: &[u8], b: &[u8]) -> bool {
    let lf = |x: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(x.len());
        for (i, c) in x.iter().enumerate() {
            if *c == b'\r' && x.get(i + 1) == Some(&b'\n') {
                continue;
            }
            out.push(*c);
        }
        out
    };
    a == b || lf(a) == lf(b)
}

/// Runs every check on the project `src` holds. With no history, only
/// what the files alone show is checked (every record is then proposed);
/// with `results`, official releases are rebuilt too.
pub fn run(
    src: &dyn Source,
    history: Option<History<'_>>,
    mut results: Option<&mut dyn Results>,
) -> Result<Vec<Finding>> {
    let config = Config::read(src)?;
    let records = Records::read(src)?;
    let none = Releases::default();
    let releases = history.as_ref().map_or(&none, |h| h.releases);
    let project: Option<ProjectId> = crate::read_json(src, MANIFEST)
        .ok()
        .flatten()
        .and_then(|v| Manifest::from_json(&ryucad_doc::doc::project::without_conflicts(&v)).ok())
        .and_then(|m| m.project);
    let parts: Vec<Id> = src
        .list("parts")?
        .into_iter()
        .filter(|(_, k)| *k == ryucad_doc::doc::source::Kind::Dir)
        .filter_map(|(n, _)| Id::parse(&n, "part").ok())
        .collect();
    let official = |e: &Entry| releases.official.get(&e.path);
    let mut out = Vec::new();
    for part in records.parts() {
        // Records that can't be read are doctor's; the rest are checked.
        let entries: Vec<(&Entry, &Record)> = records
            .of(&part)
            .filter_map(|e| Some((e, e.record.as_ref().ok()?)))
            .collect();
        for (e, r) in &entries {
            if let Some(p) = &project
                && r.project() != p
            {
                out.push(
                    Finding::new(
                        "release_project_mismatch",
                        &part,
                        Some(&e.label),
                        format!(
                            "{} names project {}, but this project is {p}",
                            e.path,
                            r.project()
                        ),
                    )
                    .with("path", &e.path)
                    .with("project", r.project())
                    .with("expected", p),
                );
            }
        }
        out.extend(sequence(&config, &part, &entries));
        let ordered = state::ordered(config.revisions, &records, &part).ok();
        let releases_of: Vec<(&Entry, &crate::record::Release)> = entries
            .iter()
            .filter_map(|(e, r)| Some((*e, r.release()?)))
            .collect();
        // Proposed releases must still be the working part.
        let content = crate::content::hash(src, &part);
        let working = props::read(src, &part);
        for (e, r) in releases_of.iter().filter(|(e, _)| official(e).is_none()) {
            let changes = match (&content, &working) {
                (Ok(c), Ok(p)) => r.changes(c, &config, p),
                _ => vec!["content".to_owned()],
            };
            if !changes.is_empty() {
                out.push(
                    Finding::new(
                        "release_stale",
                        &part,
                        Some(&e.label),
                        format!(
                            "{} proposes {part} {} but the part has changed since ({}): propose it again",
                            e.path,
                            e.label,
                            changes.join(", ")
                        ),
                    )
                    .with("path", &e.path)
                    .with("changes", &changes),
                );
            }
        }
        // A part that's gone must have been made obsolete first.
        if !parts.contains(&part)
            && let Some(list) = &ordered
            && let Some((last, _)) = list
                .iter()
                .rev()
                .find(|(e, _)| e.kind == Kind::Release && official(e).is_some())
            && !list.iter().any(|(e, _)| {
                e.kind == Kind::Obsolete && e.label == last.label && official(e).is_some()
            })
        {
            out.push(
                Finding::new(
                    "release_part_deleted",
                    &part,
                    Some(&last.label),
                    format!(
                        "part {part} is deleted, but its revision {} isn't obsolete: make it obsolete (`ryucad release obsolete {part} {}`) or restore it",
                        last.label, last.label
                    ),
                )
                .with("path", &last.path),
            );
        }
        let Some(h) = &history else {
            continue;
        };
        for (e, _) in &entries {
            let Some(o) = official(e) else {
                continue;
            };
            if releases.modified.contains_key(&e.path) {
                continue;
            }
            let at_official = h
                .repo
                .find_object(o.blob)
                .map_err(crate::history::git_error)?;
            if !same_text(&at_official.data, &e.bytes) {
                out.push(
                    Finding::new(
                        "release_modified",
                        &part,
                        Some(&e.label),
                        format!(
                            "{} differs from the record official at {}: records never change once official",
                            e.path,
                            short(&o.commit)
                        ),
                    )
                    .with("path", &e.path)
                    .with("official", &o.commit),
                );
            }
        }
        for (e, r) in &releases_of {
            let Some(o) = official(e) else {
                continue;
            };
            let then = crate::history::files(h.repo, &o.commit, h.prefix)?;
            let content = crate::content::hash(&then, &part);
            let changes = match (&content, Config::read(&then), props::read(&then, &part)) {
                (Ok(c), Ok(cfg), Ok(p)) => r.changes(c, &cfg, &p),
                _ => vec!["content".to_owned()],
            };
            if !changes.is_empty() {
                out.push(
                    Finding::new(
                        "release_hash_mismatch",
                        &part,
                        Some(&e.label),
                        format!(
                            "{} doesn't describe {part} as it was at {}, where it became official ({})",
                            e.path,
                            short(&o.commit),
                            changes.join(", ")
                        ),
                    )
                    .with("path", &e.path)
                    .with("official", &o.commit)
                    .with("changes", &changes),
                );
            }
            match releases.tag(&part, &e.label, &o.commit) {
                Tag::Ok => {}
                Tag::Missing => out.push(
                    Finding::new(
                        "release_tag_missing",
                        &part,
                        Some(&e.label),
                        format!(
                            "{part} {} has no tag {}: `ryucad release sync` makes it",
                            e.label,
                            tag_name(&part, &e.label)
                        ),
                    )
                    .with("tag", tag_name(&part, &e.label))
                    .with("official", &o.commit),
                ),
                Tag::Moved(to) => out.push(
                    Finding::new(
                        "release_tag_moved",
                        &part,
                        Some(&e.label),
                        format!(
                            "tag {} points away from {}, where {part} {} became official: `ryucad release sync` moves it back",
                            tag_name(&part, &e.label),
                            short(&o.commit),
                            e.label
                        ),
                    )
                    .with("tag", tag_name(&part, &e.label))
                    .with("official", &o.commit)
                    .with("points_at", to),
                ),
            }
            if let (Some(res), Ok(c)) = (results.as_deref_mut(), &content) {
                let rebuilt = res.bodies(&then, &part, c)?;
                if rebuilt != r.results {
                    let differ: Vec<&String> = rebuilt
                        .keys()
                        .chain(r.results.keys())
                        .filter(|f| rebuilt.get(*f) != r.results.get(*f))
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    out.push(
                        Finding::new(
                            "release_result_changed",
                            &part,
                            Some(&e.label),
                            format!(
                                "this build rebuilds {part} {} to other bodies than {} did ({})",
                                e.label,
                                r.built_with.ryucad,
                                differ
                                    .iter()
                                    .map(|f| f.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        )
                        .with("features", &differ)
                        .with("built_with", &r.built_with.ryucad),
                    );
                }
            }
        }
    }
    if history.is_some() {
        for (path, m) in &releases.modified {
            let mut seg = path.split('/').skip(1);
            let (Some(part), Some(file)) = (seg.next(), seg.next()) else {
                continue;
            };
            let (Ok(part), Some((label, _))) =
                (Id::parse(part, "part"), crate::record::parse_name(file))
            else {
                continue;
            };
            let what = if m.deleted { "deleted" } else { "changed" };
            out.push(
                Finding::new(
                    "release_modified",
                    &part,
                    Some(&label),
                    format!(
                        "{path} was official at {} and {what} at {} on the release branch: records never change once official",
                        short(&m.official),
                        short(&m.at)
                    ),
                )
                .with("path", path)
                .with("official", &m.official)
                .with("at", &m.at)
                .with("deleted", m.deleted),
            );
        }
        for key in releases.orphaned_tags() {
            let (part, label) = key.split_once('/').unwrap_or((key, ""));
            let Ok(part) = Id::parse(part, "part") else {
                continue;
            };
            out.push(
                Finding::new(
                    "release_tag_orphaned",
                    &part,
                    Some(label),
                    format!(
                        "tag ryupdm/{key} names no official release; delete it if it is wrong (`git tag -d ryupdm/{key}`)"
                    ),
                )
                .with("tag", format!("ryupdm/{key}")),
            );
        }
    }
    Ok(out)
}

/// The first twelve digits of a commit, as people read them.
fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// `release_sequence` for a part's readable records: releases follow one
/// another in the scheme, each superseding the one before, and an
/// obsolete record names a release.
fn sequence(config: &Config, part: &Id, entries: &[(&Entry, &Record)]) -> Vec<Finding> {
    let scheme = config.revisions;
    let mut out = Vec::new();
    let mut placed: Vec<(u64, &Entry, &Record)> = Vec::new();
    for (e, r) in entries {
        match label::index(scheme, &e.label) {
            Some(n) => placed.push((n, e, r)),
            None => out.push(
                Finding::new(
                    "release_sequence",
                    part,
                    Some(&e.label),
                    format!("{}: {} isn't a {} label", e.path, e.label, scheme.as_str()),
                )
                .with("path", &e.path),
            ),
        }
    }
    placed.sort_by_key(|(n, e, _)| (*n, e.kind));
    let mut previous: Option<&str> = None;
    for (_, e, r) in &placed {
        match r {
            Record::Release(rel) => {
                let want_label = match previous {
                    None => label::first(scheme),
                    Some(p) => label::next(scheme, p).expect("placed"),
                };
                let ok = rel.supersedes.as_deref() == previous && e.label == want_label;
                if !ok {
                    out.push(
                        Finding::new(
                            "release_sequence",
                            part,
                            Some(&e.label),
                            format!(
                                "{} supersedes {} but the release before it is {}, and comes after it as {want_label}",
                                e.path,
                                rel.supersedes.as_deref().unwrap_or("nothing"),
                                previous.unwrap_or("none")
                            ),
                        )
                        .with("path", &e.path)
                        .with("supersedes", &rel.supersedes)
                        .with("expected", json!({"supersedes": previous, "label": want_label})),
                    );
                }
                previous = Some(&e.label);
            }
            Record::Obsolete(_) => {
                let released = placed
                    .iter()
                    .any(|(_, x, y)| x.label == e.label && y.kind() == Kind::Release);
                if !released {
                    out.push(
                        Finding::new(
                            "release_sequence",
                            part,
                            Some(&e.label),
                            format!(
                                "{} makes {part} {} obsolete, but no record releases {}",
                                e.path, e.label, e.label
                            ),
                        )
                        .with("path", &e.path),
                    );
                }
            }
        }
    }
    out
}

/// The project's files as Git's index stages them, read on demand.
struct Staged<'a> {
    repo: &'a gix::Repository,
    files: BTreeMap<String, gix::ObjectId>,
}

impl Source for Staged<'_> {
    fn list(&self, dir: &str) -> Result<Vec<(String, ryucad_doc::doc::source::Kind)>> {
        use ryucad_doc::doc::source::Kind as K;
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut out: Vec<(String, K)> = Vec::new();
        for path in self.files.range(prefix.clone()..).map(|(p, _)| p) {
            let Some(rest) = path.strip_prefix(&prefix) else {
                break;
            };
            let entry = match rest.split_once('/') {
                Some((name, _)) => (name.to_owned(), K::Dir),
                None => (rest.to_owned(), K::File),
            };
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.files.get(path) {
            None => Ok(None),
            Some(id) => Ok(Some(
                self.repo
                    .find_object(*id)
                    .map_err(crate::history::git_error)?
                    .data
                    .clone(),
            )),
        }
    }
}

/// The commit guard: what a commit of Git's index would get wrong about
/// releases, for the project at `prefix`. It reads only the index, `HEAD`
/// and the release branch's tip, and rebuilds nothing:
///
/// - `release_stale`: a part staged with a proposed (not official)
///   release that no longer describes it, or a release staged that
///   doesn't describe its staged part;
/// - `release_sequence` and `release_project_mismatch` for records staged
///   new or changed;
/// - `release_modified`: an official record staged changed or deleted.
///
/// A commit that stages no record and no file of a part with records is
/// passed at once. Withdrawing a proposal (deleting its record) is fine.
pub fn staged(repo: &gix::Repository, prefix: &str) -> Result<Vec<Finding>> {
    use crate::history::git_error;
    use ryucad_doc::doc::project::RELEASES;
    let mut index = BTreeMap::new();
    if let Ok(file) = repo.open_index() {
        for e in file.entries() {
            if e.stage() != gix::index::entry::Stage::Unconflicted {
                continue;
            }
            let path = e.path(&file).to_string();
            if let Some(rel) = path.strip_prefix(prefix) {
                index.insert(rel.to_owned(), e.id);
            }
        }
    }
    let head: BTreeMap<String, gix::ObjectId> = match repo.head_commit() {
        Ok(c) => {
            let mut all = crate::history::records_at(repo, &c.id.to_string(), prefix)?;
            // Part files, to see which parts the commit touches.
            let tree = c.tree_id().map_err(git_error)?.detach();
            if let Some(parts) = crate::history::entry(repo, tree, prefix, "parts")? {
                crate::history::blobs(repo, parts, "parts/", &mut all)?;
            }
            all
        }
        Err(_) => BTreeMap::new(),
    };
    let changed = |p: &String| index.get(p) != head.get(p);
    let paths: std::collections::BTreeSet<&String> = index.keys().chain(head.keys()).collect();
    let mut records_changed: Vec<String> = Vec::new();
    let mut parts_touched: std::collections::BTreeSet<Id> = Default::default();
    for p in paths {
        if !changed(p) {
            continue;
        }
        let mut seg = p.split('/');
        match (seg.next(), seg.next(), seg.next()) {
            (Some(RELEASES), Some(part), Some(file)) if seg.next().is_none() => {
                if let (Ok(id), Some(_)) =
                    (Id::parse(part, "part"), crate::record::parse_name(file))
                {
                    records_changed.push(p.clone());
                    parts_touched.insert(id);
                }
            }
            (Some("parts"), Some(part), Some(_)) => {
                if let Ok(id) = Id::parse(part, "part") {
                    parts_touched.insert(id);
                }
            }
            _ => {}
        }
    }
    let src = Staged { repo, files: index };
    let records = Records::read(&src)?;
    let has_records = |p: &Id| records.of(p).next().is_some();
    if records_changed.is_empty() && !parts_touched.iter().any(has_records) {
        return Ok(Vec::new());
    }
    let config = Config::read(&src)?;
    let official: BTreeMap<String, gix::ObjectId> =
        match crate::history::branch(repo, &config.release_branch)? {
            Some(b) => crate::history::records_at(repo, &b.tip, prefix)?,
            None => BTreeMap::new(),
        };
    let project = crate::read_json(&src, MANIFEST)
        .ok()
        .flatten()
        .and_then(|v| Manifest::from_json(&v).ok())
        .and_then(|m| m.project);
    let mut out = Vec::new();
    for path in &records_changed {
        let Some(blob) = official.get(path) else {
            continue;
        };
        if src.files.get(path) == Some(blob) {
            continue;
        }
        let (part, label) = path_part_label(path);
        let what = if src.files.contains_key(path) {
            "changes"
        } else {
            "deletes"
        };
        out.push(
            Finding::new(
                "release_modified",
                &part,
                Some(&label),
                format!(
                    "this commit {what} {path}, which is official: records never change once official"
                ),
            )
            .with("path", path),
        );
    }
    for part in &parts_touched {
        let entries: Vec<(&Entry, &Record)> = records
            .of(part)
            .filter_map(|e| Some((e, e.record.as_ref().ok()?)))
            .collect();
        let mine = |e: &Entry| records_changed.contains(&e.path);
        for (e, r) in &entries {
            if mine(e)
                && let Some(p) = &project
                && r.project() != p
            {
                out.push(
                    Finding::new(
                        "release_project_mismatch",
                        part,
                        Some(&e.label),
                        format!(
                            "{} names project {}, but this project is {p}",
                            e.path,
                            r.project()
                        ),
                    )
                    .with("path", &e.path),
                );
            }
        }
        out.extend(sequence(&config, part, &entries).into_iter().filter(|f| {
            f.details["path"]
                .as_str()
                .is_some_and(|p| records_changed.iter().any(|c| c == p))
        }));
        let content = crate::content::hash(&src, part);
        let working = props::read(&src, part);
        for (e, r) in &entries {
            let Some(rel) = r.release() else {
                continue;
            };
            if official.contains_key(&e.path) {
                continue;
            }
            let changes = match (&content, &working) {
                (Ok(c), Ok(p)) => rel.changes(c, &config, p),
                _ => vec!["content".to_owned()],
            };
            if !changes.is_empty() {
                out.push(
                    Finding::new(
                        "release_stale",
                        part,
                        Some(&e.label),
                        format!(
                            "{} proposes {part} {} but the part being committed differs ({}): withdraw the proposal (delete it) and propose again, or undo the change",
                            e.path,
                            e.label,
                            changes.join(", ")
                        ),
                    )
                    .with("path", &e.path)
                    .with("changes", &changes),
                );
            }
        }
    }
    Ok(out)
}

/// The part and label a record's path names.
fn path_part_label(path: &str) -> (Id, String) {
    let mut seg = path.split('/').skip(1);
    let part = Id::parse(seg.next().unwrap_or_default(), "part").expect("a record's path");
    let (label, _) =
        crate::record::parse_name(seg.next().unwrap_or_default()).expect("a record's name");
    (part, label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::{commit, git, put};
    use crate::state::tests::{add, obsolete, project, release_now};
    use ryucad_doc::doc::source::Folder;
    use std::path::Path;

    fn codes(found: &[Finding]) -> Vec<(&'static str, String)> {
        found
            .iter()
            .map(|f| (f.code, f.label.clone().unwrap_or_default()))
            .collect()
    }

    fn write(dir: &Path, files: &BTreeMap<String, Vec<u8>>) {
        for (p, b) in files {
            put(dir, p, std::str::from_utf8(b).unwrap());
        }
    }

    /// The checks on the project in `dir`, with its history.
    fn checked(dir: &Path) -> Vec<Finding> {
        let repo = gix::open(dir).unwrap();
        let src = Folder(dir);
        let releases = Releases::read(&repo, "", &Config::read(&src).unwrap()).unwrap();
        let h = History {
            repo: &repo,
            prefix: "",
            releases: &releases,
        };
        run(&src, Some(h), None).unwrap()
    }

    #[test]
    fn the_files_alone_show_stale_proposals_and_broken_sequences() {
        let mut f = project();
        let r = release_now(&f, "A", None);
        add(&mut f, &r);
        assert!(run(&f, None, None).unwrap().is_empty());
        // The part changes after it was proposed.
        f.insert("parts/p/features/a.json".into(), b"{\"x\": 2}".to_vec());
        let found = run(&f, None, None).unwrap();
        assert_eq!(codes(&found), [("release_stale", "A".into())]);
        assert_eq!(found[0].details["changes"], json!(["content"]));
        let v = found[0].to_json();
        assert_eq!(
            (&v["source"], &v["severity"], &v["part"]),
            (&json!("release"), &json!("error"), &json!("p"))
        );
        // C supersedes A, skipping B; D is obsolete with no release; 1
        // isn't a letter; another project's record.
        let mut f = project();
        let r = release_now(&f, "A", None);
        add(&mut f, &r);
        let c = release_now(&f, "C", Some("A"));
        add(&mut f, &c);
        add(&mut f, &obsolete("D"));
        let mut one = release_now(&f, "B", Some("A"));
        if let Record::Release(x) = &mut one {
            x.label = "1".into();
            x.project = ProjectId::parse("ffffffffffffffffffffffffffffffff").unwrap();
        }
        add(&mut f, &one);
        let found = run(&f, None, None).unwrap();
        assert_eq!(
            codes(&found),
            [
                ("release_project_mismatch", "1".into()),
                ("release_sequence", "1".into()),
                ("release_sequence", "C".into()),
                ("release_sequence", "D".into()),
            ]
        );
        assert_eq!(
            found[2].details["expected"],
            json!({"supersedes": "A", "label": "B"})
        );
    }

    #[test]
    fn official_releases_are_checked_against_history_and_tags() {
        let dir = crate::history::tests::repo();
        let d = dir.path();
        let mut f = project();
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        write(d, &f);
        let official = commit(d, "release A");
        assert_eq!(codes(&checked(d)), [("release_tag_missing", "A".into())]);
        git(d, &["tag", "ryupdm/p/A", &official]);
        assert!(checked(d).is_empty());
        // Moved, and one no release accounts for.
        let later = commit(d, "later");
        git(d, &["tag", "-f", "ryupdm/p/A", &later]);
        git(d, &["tag", "ryupdm/p/B", &later]);
        let found = checked(d);
        assert_eq!(
            codes(&found),
            [
                ("release_tag_moved", "A".into()),
                ("release_tag_orphaned", "B".into())
            ]
        );
        assert_eq!(found[0].details["points_at"], later);
        git(d, &["tag", "-d", "ryupdm/p/B"]);
        git(d, &["tag", "-f", "ryupdm/p/A", &official]);
        // A record edited in the checkout, then on the branch.
        let path = a.path();
        let text = std::fs::read_to_string(d.join(&path)).unwrap();
        put(d, &path, &text.replace("agent", "someone"));
        assert_eq!(codes(&checked(d)), [("release_modified", "A".into())]);
        commit(d, "edit A");
        let found = checked(d);
        assert_eq!(codes(&found), [("release_modified", "A".into())]);
        assert_eq!(found[0].details["deleted"], false);
        assert_eq!(found[0].details["official"], official);
    }

    #[test]
    fn a_record_that_never_described_its_part() {
        let dir = crate::history::tests::repo();
        let d = dir.path();
        let mut f = project();
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        // The part changes in the commit that makes the record official.
        f.insert("parts/p/features/a.json".into(), b"{\"x\": 3}".to_vec());
        write(d, &f);
        let at = commit(d, "release A, edited");
        git(d, &["tag", "ryupdm/p/A", &at]);
        let found = checked(d);
        assert_eq!(codes(&found), [("release_hash_mismatch", "A".into())]);
        assert_eq!(found[0].details["changes"], json!(["content"]));
    }

    #[test]
    fn a_deleted_part_must_be_obsolete_first() {
        let dir = crate::history::tests::repo();
        let d = dir.path();
        let mut f = project();
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        write(d, &f);
        let at = commit(d, "release A");
        git(d, &["tag", "ryupdm/p/A", &at]);
        std::fs::remove_dir_all(d.join("parts/p")).unwrap();
        commit(d, "delete p");
        assert_eq!(codes(&checked(d)), [("release_part_deleted", "A".into())]);
        // Proposed obsolete isn't enough; official is.
        let o = obsolete("A");
        let mut f = BTreeMap::new();
        add(&mut f, &o);
        write(d, &f);
        assert_eq!(codes(&checked(d)), [("release_part_deleted", "A".into())]);
        commit(d, "obsolete A");
        assert!(checked(d).is_empty());
    }

    struct Fixed(BTreeMap<String, String>, usize);

    impl Results for Fixed {
        fn bodies(
            &mut self,
            _: &dyn Source,
            _: &Id,
            content: &str,
        ) -> Result<BTreeMap<String, String>> {
            assert_eq!(content.len(), 64);
            self.1 += 1;
            Ok(self.0.clone())
        }
    }

    #[test]
    fn rebuilt_results_are_compared_on_request() {
        let dir = crate::history::tests::repo();
        let d = dir.path();
        let mut f = project();
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        write(d, &f);
        let at = commit(d, "release A");
        git(d, &["tag", "ryupdm/p/A", &at]);
        let repo = gix::open(d).unwrap();
        let src = Folder(d);
        let releases = Releases::read(&repo, "", &Config::default()).unwrap();
        let mut same = Fixed(BTreeMap::new(), 0);
        let h = || History {
            repo: &repo,
            prefix: "",
            releases: &releases,
        };
        assert!(run(&src, Some(h()), Some(&mut same)).unwrap().is_empty());
        assert_eq!(same.1, 1);
        let mut other = Fixed([("a".to_owned(), "h".repeat(64))].into(), 0);
        let found = run(&src, Some(h()), Some(&mut other)).unwrap();
        assert_eq!(codes(&found), [("release_result_changed", "A".into())]);
        assert_eq!(found[0].severity(), "warning");
        assert_eq!(found[0].details["features"], json!(["a"]));
    }

    /// Stages everything in `dir` and runs the guard.
    fn guard(dir: &Path) -> Vec<(&'static str, String)> {
        git(dir, &["add", "-A"]);
        let repo = gix::open(dir).unwrap();
        codes(&staged(&repo, "").unwrap())
    }

    #[test]
    fn the_guard_passes_a_proposal_and_refuses_a_stale_one() {
        let dir = crate::history::tests::repo();
        let d = dir.path();
        let mut f = project();
        write(d, &f);
        commit(d, "a part");
        git(d, &["switch", "-q", "-c", "work"]);
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        write(d, &f);
        assert!(guard(d).is_empty());
        commit(d, "propose A");
        // Nothing about releases staged: passed at once.
        put(d, "notes.txt", "x");
        assert!(guard(d).is_empty());
        // Editing the proposed part is refused...
        put(d, "parts/p/features/a.json", "{\"x\": 2}");
        assert_eq!(guard(d), [("release_stale", "A".into())]);
        // ...until the proposal is withdrawn.
        std::fs::remove_file(d.join(a.path())).unwrap();
        assert!(guard(d).is_empty());
    }

    #[test]
    fn the_guard_keeps_official_records_as_they_are() {
        let dir = crate::history::tests::repo();
        let d = dir.path();
        let mut f = project();
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        write(d, &f);
        commit(d, "release A on main");
        // The part may move on: it is work towards B.
        put(d, "parts/p/features/a.json", "{\"x\": 2}");
        assert!(guard(d).is_empty());
        git(d, &["checkout", "-q", "--", "."]);
        git(d, &["reset", "-q"]);
        let path = a.path();
        let text = std::fs::read_to_string(d.join(&path)).unwrap();
        put(d, &path, &text.replace("agent", "someone"));
        assert_eq!(guard(d), [("release_modified", "A".into())]);
        git(d, &["reset", "-q", "--hard"]);
        std::fs::remove_file(d.join(&path)).unwrap();
        let repo = gix::open(d).unwrap();
        git(d, &["add", "-A"]);
        let found = staged(&repo, "").unwrap();
        assert_eq!(codes(&found), [("release_modified", "A".into())]);
        assert!(found[0].message.contains("deletes"));
    }

    #[test]
    fn the_guard_checks_what_new_records_say() {
        let dir = crate::history::tests::repo();
        let d = dir.path();
        let mut f = project();
        write(d, &f);
        commit(d, "a part");
        let mut b = release_now(&f, "B", Some("A"));
        if let Record::Release(x) = &mut b {
            x.project = ProjectId::parse("ffffffffffffffffffffffffffffffff").unwrap();
        }
        add(&mut f, &b);
        write(d, &f);
        assert_eq!(
            guard(d),
            [
                ("release_project_mismatch", "B".into()),
                ("release_sequence", "B".into())
            ]
        );
    }

    #[test]
    fn the_guard_reads_a_project_below_the_root() {
        let dir = crate::history::tests::repo();
        let d = dir.path();
        let mut f = project();
        let a = release_now(&f, "A", None);
        add(&mut f, &a);
        f.insert("parts/p/features/a.json".into(), b"{\"x\": 5}".to_vec());
        for (p, b) in &f {
            put(d, &format!("cad/{p}"), std::str::from_utf8(b).unwrap());
        }
        git(d, &["add", "-A"]);
        let repo = gix::open(d).unwrap();
        assert_eq!(
            codes(&staged(&repo, "cad/").unwrap()),
            [("release_stale", "A".into())]
        );
        assert!(staged(&repo, "").unwrap().is_empty());
    }
}
